use crate::artifact::assembly::{self, AssembledWorkload, ResolvedSandbox};
use crate::artifact::fileset::{FilesetBudget, fileset_runtime_specs_with_budget};
use crate::artifact::{RunPath, dispatch_run};
use crate::image::{RealRegistry, Registry, registry_auth_for};
use crate::runtime_layer::RuntimeFileSpec;
use anyhow::{Context, Result};
use lns_ipc::ArtifactInspection;
use oci_client::Reference;

/// A resolved sandbox ready to boot: the assembled workload plus the guest-write specs that materialize its filesets into the microVM.
pub(crate) struct SandboxPlan {
    pub workload: AssembledWorkload,
    pub fileset_specs: Vec<RuntimeFileSpec>,
}

/// What a published run reference resolves to: everything its manifest, its pins and the merge decide, with nothing recorded, nothing disclosed and nothing written — so the answer is known before a run is registered.
pub(crate) struct ResolvedForRun {
    reference: String,
    pub(crate) digest: String,
    resolved: crate::artifact::assembly::ResolvedSandbox,
}

/// Peek a run reference's manifest and, when it is a published sandbox, resolve it; a plain image returns `None` so the caller runs it directly (a bare `verify_sandbox` reference that resolves to a plain image is refused as "not a sandbox").
pub(crate) async fn resolve_for_run(
    image_ref: &str,
    verify_sandbox: bool,
    mixins: &[String],
    decisions: Option<&std::path::Path>,
) -> Result<Option<ResolvedForRun>> {
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
        RunPath::SingleImage => {
            crate::artifact::mixin::refuse_mixins_without_a_document(mixins)?;
            Ok(None)
        }
        RunPath::Sandbox => {
            crate::artifact::mixin::require_pinned_extras(mixins)?;
            let resolution = crate::artifact::mixin::resolve(
                config_json.as_bytes(),
                mixins,
                &crate::artifact::mixin::Locator::Reference(image_ref.to_string()),
                &RegistryMixins,
                local_source(decisions)?,
            )
            .await
            .with_context(|| format!("resolving {image_ref}"))?;
            let packed = packed_filesets(&resolution, Some((image_ref, &manifest)))
                .map_err(|problems| refusal(image_ref, problems))?;
            let resolved = crate::artifact::with_authored_baseline(
                crate::artifact::plan_published_sandbox(&resolution.document, image_ref, &packed)?,
                &resolution.authored_egress,
            );
            Ok(Some(ResolvedForRun {
                reference: image_ref.to_string(),
                digest,
                resolved,
            }))
        }
    }
}

/// Turn what a reference resolved to into a bootable plan: the run it is recorded against, the policy it discloses, and the filesets it materializes all belong to a run that has started.
pub(crate) async fn plan_resolved(
    resolved: ResolvedForRun,
    run_id: &str,
    microvm: &str,
    denied_host_paths: &[String],
) -> Result<SandboxPlan> {
    let ResolvedForRun {
        reference,
        digest,
        resolved,
    } = resolved;
    record_sandbox_run(run_id, microvm, &reference, &digest, &resolved);
    crate::image_store::record_artifact_run(&reference, &digest, &resolved.base_image)
        .await
        .with_context(|| format!("recording the sandbox index for {reference}"))?;
    disclose_effective_policy(resolved.policy.as_ref());
    let problems = crate::artifact::published_fileset_problems(&resolved);
    if !problems.is_empty() {
        return Err(refusal(&reference, problems));
    }
    let mut materialized = materialize_filesets(&resolved).await?;
    crate::artifact::fileset::host_fileset_specs(
        &RealSnapshotDir,
        &resolved.host_filesets,
        denied_host_paths,
        &mut materialized,
    )?;
    let fileset_specs = materialized.into_specs();
    Ok(SandboxPlan {
        workload: assembly::assemble(&resolved),
        fileset_specs,
    })
}

/// Correlate the resolved document's packed filesets with the layers that carry them: the sandbox's own artifact when the run is a published reference, plus every mixin's own artifact.
fn packed_filesets(
    resolution: &crate::artifact::mixin::Resolution,
    own: Option<(&str, &oci_client::manifest::OciImageManifest)>,
) -> Result<crate::artifact::PackedFilesets, Vec<String>> {
    let mut carriers = resolution.carriers.clone();
    if let Some((reference, manifest)) = own {
        carriers.insert(
            lns_artifact::merge::ROOT_LABEL.to_string(),
            crate::artifact::Carrier {
                reference: reference.to_string(),
                layers: crate::artifact::fileset::packed_layers(manifest),
            },
        );
    }
    crate::artifact::correlate_packed_filesets(
        &resolution.fileset_origins,
        &resolution.declared_path_filesets,
        &carriers,
    )
}

fn refusal(image_ref: &str, problems: Vec<String>) -> anyhow::Error {
    anyhow::anyhow!("refusing to run {image_ref}: {}", problems.join("; "))
}

/// Plan a local `lns.yaml` definition into a bootable workload, disclosing its shipped policy exactly like a published sandbox run. `authored_egress` is what the preflight resolved from every source but this directory's own, absent when the run resolved nothing to layer on; a published mixin it layers on still brings its filesets packed in its own artifact.
pub(crate) async fn plan_local(
    definition_json: &str,
    authored_egress: Option<&str>,
    packed: &crate::artifact::PackedFilesets,
) -> Result<SandboxPlan> {
    let mut resolved = crate::artifact::plan_local_sandbox(definition_json.as_bytes(), packed)?;
    if let Some(authored) = authored_egress {
        resolved = crate::artifact::with_authored_baseline(
            resolved,
            &crate::artifact::authored_egress(authored)?,
        );
    }
    disclose_effective_policy(resolved.policy.as_ref());
    let mut materialized = materialize_filesets(&resolved).await?;
    crate::artifact::fileset::local_fileset_specs(
        &RealSnapshotDir,
        &resolved.local_filesets,
        &mut materialized,
    )?;
    crate::artifact::fileset::host_fileset_specs(
        &RealSnapshotDir,
        &resolved.host_filesets,
        &[],
        &mut materialized,
    )?;
    Ok(SandboxPlan {
        workload: assembly::assemble(&resolved),
        fileset_specs: materialized.into_specs(),
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

impl crate::artifact::fileset::HostFileProbe for RealSnapshotDir {
    fn home(&self) -> Option<std::path::PathBuf> {
        dirs::home_dir()
    }

    /// `metadata`, not `symlink_metadata`: a stow/chezmoi dotfile is a symlink, and the read that follows this seeds from the target too.
    fn stat(
        &self,
        path: &std::path::Path,
    ) -> std::io::Result<Option<crate::artifact::fileset::HostFileFacts>> {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(metadata) => Ok(Some(crate::artifact::fileset::HostFileFacts {
                mode: metadata.permissions().mode() & 0o777,
                is_regular_file: metadata.is_file(),
            })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

/// Refuse a launch whose definition declares a `spec.connectors` id this machine's catalog cannot arm; fail-fast at boot instead of an opaque mid-run credential miss. A declared credential names no connector, so it can never be unknown here.
pub(crate) fn refuse_unknown_connectors(policy: Option<&lns_policy::Policy>) -> Result<()> {
    let declared: Vec<String> = policy.map(|p| p.connectors.clone()).unwrap_or_default();
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

/// The directory's own decisions as a merge source, read off this machine; a run that named no decisions file resolves without one.
fn local_source(
    decisions: Option<&std::path::Path>,
) -> Result<Option<crate::artifact::mixin::LocalSource>> {
    let Some(path) = decisions else {
        return Ok(None);
    };
    crate::artifact::mixin::LocalSource::read(
        crate::artifact::mixin_dir::read_local_mixin(&RealMixinDir, path)?,
        crate::artifact::mixin::Locator::Local(lns_artifact::sandbox::fold_path(path)),
    )
}

/// Reads a directory mixin off this machine's filesystem.
pub(crate) struct RealMixinDir;

impl crate::artifact::mixin_dir::MixinDir for RealMixinDir {
    fn is_dir(&self, path: &std::path::Path) -> bool {
        path.is_dir()
    }

    fn read(&self, path: &std::path::Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
}

/// Pulls a declared mixin's document from the registry it names, through the manifest cache so a digest-pinned graph pulled once resolves offline.
pub(crate) struct RegistryMixins;

impl crate::artifact::mixin::MixinSource for RegistryMixins {
    async fn fetch(
        &self,
        locator: &crate::artifact::mixin::Locator,
    ) -> Result<crate::artifact::mixin::FetchedMixin> {
        match locator {
            crate::artifact::mixin::Locator::Reference(reference) => {
                let registry = crate::image::caching_registry_for(reference)?;
                crate::image::pull_mixin_with(&registry, reference).await
            }
            crate::artifact::mixin::Locator::Local(dir) => {
                crate::artifact::mixin_dir::read_path_mixin(&RealMixinDir, dir)
            }
        }
    }
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

/// Pull each packed fileset's layer out of the artifact that carries it and expand it into guest-write specs, so the files a sandbox or mixin shipped land in the microVM at their mount paths.
async fn materialize_filesets(
    resolved: &ResolvedSandbox,
) -> Result<crate::artifact::fileset::MaterializedFilesets> {
    let content_store =
        crate::content_store::ContentStore::new(crate::cache::root()?.join("content"));
    let mut out = crate::artifact::fileset::MaterializedFilesets::default();
    for fileset in &resolved.packed_filesets {
        let layer = pull_packed_layer(&fileset.source, &content_store)
            .await
            .with_context(|| format!("materializing the fileset at {}", fileset.mount_path))?;
        let file = std::fs::File::open(&layer)
            .with_context(|| format!("opening fileset layer {}", layer.display()))?;
        let specs = fileset_runtime_specs_with_budget(
            &fileset.mount_path,
            flate2::read::GzDecoder::new(file),
            &content_store,
            &mut FilesetBudget::new(),
        )?;
        out.absorb(fileset.owner, &fileset.mount_path, specs);
    }
    crate::artifact::fileset::inline_fileset_specs(&resolved.inline_filesets, &mut out);
    Ok(out)
}

async fn pull_packed_layer(
    source: &crate::artifact::PackedSource,
    content_store: &crate::content_store::ContentStore,
) -> Result<std::path::PathBuf> {
    let parsed: Reference = source
        .reference
        .parse()
        .with_context(|| format!("invalid artifact reference {}", source.reference))?;
    let registry = RealRegistry::for_reference(&parsed, registry_auth_for(&source.reference));
    pull_packed_layer_with(&registry, &parsed, &source.layer, content_store).await
}

/// Pull one packed fileset layer, verified against the digest its artifact's manifest declared, so what materializes is what the approved artifact carries; a layer already in the content store is not fetched again, and a stream failure leaves no partial content staged.
async fn pull_packed_layer_with<R: Registry>(
    registry: &R,
    parsed: &Reference,
    layer: &crate::artifact::PackedLayer,
    content_store: &crate::content_store::ContentStore,
) -> Result<std::path::PathBuf> {
    crate::artifact::fileset::validate_packed_layer_size(
        layer,
        lns_artifact::build::MAX_FILESET_BYTES,
    )?;
    if content_store.contains(&layer.digest)? {
        return content_store.path_for(&layer.digest);
    }
    let descriptor = oci_client::manifest::OciDescriptor {
        media_type: lns_artifact::build::FILESET_LAYER_MEDIA_TYPE.to_string(),
        digest: layer.digest.clone(),
        size: i64::try_from(layer.size).unwrap_or(i64::MAX),
        ..Default::default()
    };
    let staged = content_store.staging_path()?;
    if let Err(error) = registry
        .pull_blob_to_path(parsed, &descriptor, layer.size, &staged, &|_| {})
        .await
    {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    let installed = content_store
        .commit_verified(staged, &layer.digest, layer.size)
        .with_context(|| format!("verifying fileset layer {}", layer.digest))?;
    Ok(installed.path)
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

/// Disclose the sandbox's shipped network policy and declared connectors at boot: name the policy as the source your own decisions layer over (warning if it is over-broad), and disclose that declared connectors seed placeholders but are offered on first use, never armed automatically.
fn disclose_effective_policy(policy: Option<&lns_policy::Policy>) {
    let Some(policy) = policy else {
        return;
    };
    if policy.network != lns_policy::NetworkPolicy::default() {
        crate::log::info!(
            "policy",
            "this sandbox ships a network policy; it governs the run except where your own lns-local-mixin.yaml decides otherwise"
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

/// Resolve a local definition's mixins the same way a published one's resolve, so what a local run boots is the merged document and not the file the CLI happened to parse.
pub(crate) async fn resolve_definition(
    definition: &str,
    project_dir: &str,
    mixins: &[String],
    decisions: Option<&std::path::Path>,
) -> Result<lns_ipc::Response> {
    let project_dir = std::path::Path::new(project_dir);
    crate::artifact::mixin::require_a_rooted_project_dir(project_dir)?;
    let home = crate::artifact::mixin::Locator::Local(project_dir.join("lns.yaml"));
    let resolution = crate::artifact::mixin::resolve(
        definition.as_bytes(),
        mixins,
        &home,
        &RegistryMixins,
        local_source(decisions)?,
    )
    .await
    .with_context(|| format!("resolving the definition in {}", project_dir.display()))?;
    let packed_filesets = packed_filesets(&resolution, None)
        .map_err(|problems| anyhow::anyhow!(problems.join("; ")))?
        .into_iter()
        .map(|(mount_path, source)| lns_ipc::PackedFilesetSource {
            mount_path,
            reference: source.reference,
            digest: source.layer.digest,
            size: source.layer.size,
        })
        .collect();
    Ok(lns_ipc::Response::DefinitionResolved {
        definition: String::from_utf8(resolution.document)
            .context("the resolved definition is not utf-8")?,
        mixins: resolution.mixins,
        pinned_mixins: resolution.pinned_extra,
        contributions: crate::artifact::mixin::on_the_wire(&resolution.contributions),
        authored_egress: serde_json::to_string(&resolution.authored_egress)
            .context("serializing the egress this run resolved")?,
        packed_filesets,
    })
}

pub(crate) async fn inspect(
    image_ref: &str,
    mixins: &[String],
    decisions: Option<&std::path::Path>,
) -> Result<ArtifactInspection> {
    let reference: Reference = image_ref
        .parse()
        .with_context(|| format!("invalid image reference {image_ref}"))?;
    let registry = crate::image::caching_registry_for(image_ref)?;
    let (manifest, digest, config_json) = registry
        .pull_manifest_and_config(&reference)
        .await
        .with_context(|| format!("inspecting {image_ref}"))?;
    crate::image::verify_digest_pin(&reference, &digest, image_ref)?;
    // The preflight view is where a run's mounts and ports come from, so it has to describe the resolved sandbox — otherwise what a mixin contributes to them is dropped without a word.
    let resolution = crate::artifact::mixin::resolve_if_a_sandbox(
        manifest.artifact_type.as_deref(),
        Some(manifest.config.media_type.as_str()),
        config_json.as_bytes(),
        mixins,
        &crate::artifact::mixin::Locator::Reference(image_ref.to_string()),
        &RegistryMixins,
        local_source(decisions)?,
    )
    .await
    .with_context(|| format!("resolving {image_ref}"))?;
    crate::artifact::inspect::project_inspection(
        image_ref,
        digest,
        manifest.artifact_type.as_deref(),
        &manifest.config.media_type,
        &resolution,
        lns_artifact::resources::host::probe(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::fileset::{HostFileFacts, HostFileProbe};
    use oci_client::manifest::{OciDescriptor, OciImageManifest};
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn a_symlinked_host_file_reads_as_the_file_it_points_at() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("dotfiles").join("gitconfig");
        std::fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        std::fs::write(&target, b"[user]").expect("write");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).expect("chmod");
        let link = dir.path().join(".gitconfig");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert_eq!(
            RealSnapshotDir.stat(&link).expect("stat"),
            Some(HostFileFacts {
                mode: 0o640,
                is_regular_file: true
            }),
            "stow, chezmoi and home-manager all leave ~/.gitconfig a symlink, and the read that follows this one seeds from the target; refusing the link here refuses the whole run and leaves `optional` powerless"
        );
    }

    #[test]
    fn a_symlink_with_no_target_counts_as_an_absent_host_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let link = dir.path().join(".gitconfig");
        std::os::unix::fs::symlink(dir.path().join("gone"), &link).expect("symlink");
        assert_eq!(
            RealSnapshotDir.stat(&link).expect("stat"),
            None,
            "nothing is behind the link, so there is no file to seed: absent is the honest answer, and an optional fileset skips it instead of refusing the run"
        );
    }

    #[test]
    fn a_directory_at_a_host_path_is_still_reported_as_not_a_regular_file() {
        assert_eq!(
            RealSnapshotDir
                .stat(tempfile::tempdir().expect("tempdir").path())
                .expect("stat")
                .map(|facts| facts.is_regular_file),
            Some(false),
            "following the link must not soften the genuine case: a directory is not a file the guest can be seeded with"
        );
    }

    struct StreamingRegistry {
        blob: Vec<u8>,
        fail: bool,
        streamed: AtomicBool,
    }

    impl StreamingRegistry {
        fn layer(blob: Vec<u8>, fail: bool) -> Self {
            Self {
                blob,
                fail,
                streamed: AtomicBool::new(false),
            }
        }

        fn packed(&self) -> crate::artifact::PackedLayer {
            crate::artifact::PackedLayer {
                digest: format!("sha256:{}", hex::encode(Sha256::digest(&self.blob))),
                size: self.blob.len() as u64,
            }
        }
    }

    impl Registry for StreamingRegistry {
        async fn pull_manifest_and_config(
            &self,
            _reference: &Reference,
        ) -> Result<(OciImageManifest, String, String)> {
            anyhow::bail!(
                "a packed fileset layer is addressed by the manifest its own artifact was peeked from, so materializing one must not fetch a second manifest"
            )
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
        "registry.example.test/team/sandbox@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .parse()
            .unwrap()
    }

    #[tokio::test]
    async fn a_packed_layer_streams_to_verified_content_without_buffering_a_blob_vec() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::content_store::ContentStore::new(dir.path());
        let registry = StreamingRegistry::layer(vec![7; 512 * 1024], false);

        let path =
            pull_packed_layer_with(&registry, &pinned_reference(), &registry.packed(), &store)
                .await
                .unwrap();

        assert!(registry.streamed.load(Ordering::Relaxed));
        assert_eq!(std::fs::read(&path).unwrap(), registry.blob);
    }

    #[tokio::test]
    async fn a_layer_already_in_the_content_store_is_not_fetched_again() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::content_store::ContentStore::new(dir.path());
        let registry = StreamingRegistry::layer(vec![7; 1024], false);
        let installed = store.install_from_bytes(&registry.blob).unwrap();

        let path =
            pull_packed_layer_with(&registry, &pinned_reference(), &registry.packed(), &store)
                .await
                .unwrap();

        assert_eq!(path, installed.path);
        assert!(
            !registry.streamed.load(Ordering::Relaxed),
            "one directory packs to one digest however many artifacts carry it, which is the whole point of content-addressing the layer"
        );
    }

    #[tokio::test]
    async fn a_layer_beyond_the_byte_ceiling_is_refused_before_it_is_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::content_store::ContentStore::new(dir.path());
        let registry = StreamingRegistry::layer(vec![7; 1024], false);
        let oversized = crate::artifact::PackedLayer {
            size: lns_artifact::build::MAX_FILESET_BYTES + 1,
            ..registry.packed()
        };

        let err = pull_packed_layer_with(&registry, &pinned_reference(), &oversized, &store)
            .await
            .unwrap_err();

        assert!(format!("{err:#}").contains("byte limit"), "got: {err:#}");
        assert!(!registry.streamed.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn a_layer_whose_bytes_do_not_hash_to_the_declared_digest_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::content_store::ContentStore::new(dir.path());
        let registry = StreamingRegistry::layer(vec![7; 1024], false);
        let tampered = crate::artifact::PackedLayer {
            digest: format!("sha256:{}", "b".repeat(64)),
            ..registry.packed()
        };

        let err = pull_packed_layer_with(&registry, &pinned_reference(), &tampered, &store)
            .await
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("verifying fileset layer"),
            "the layer digest is what ties the files to the artifact the user approved; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_failed_layer_stream_leaves_no_partial_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::content_store::ContentStore::new(dir.path());
        let registry = StreamingRegistry::layer(vec![7; 128 * 1024], true);

        let err =
            pull_packed_layer_with(&registry, &pinned_reference(), &registry.packed(), &store)
                .await
                .unwrap_err();

        assert!(format!("{err:#}").contains("registry stream failed"));
        let entries = std::fs::read_dir(dir.path().join("sha256"))
            .unwrap()
            .count();
        assert_eq!(entries, 0);
    }
}
