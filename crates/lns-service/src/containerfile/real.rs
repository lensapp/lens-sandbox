//! The production wiring of the executor: the registry a `FROM` resolves through, the build guest a
//! `RUN` runs in, the host directory a `COPY` reads, and the local store every commit lands in.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use lns_ipc::{RunImageArgs, WireFrame};
use tokio::sync::mpsc::Sender;

use crate::image::manifest_cache::ManifestCache;
use crate::log;
use crate::oci_layer_cache::LayerCache;

use super::cache::{BuildCache, CacheFs, Entry, Kind};
use super::context::{ContextFs, EntryKind, Meta};
use super::executor::{self, Base, BuildHost, Commit, CopyStep, RunOutcome, RunStep};
use super::ext4_upper::Ext4Upper;
use super::image::ParentImage;
use super::import::LocalStore;
use super::key;
use super::locate;
use super::step;
use super::upper::{self, ChangeSet};

/// Where the reference of the image a run built lands, in the run's own directory: two runs ending together would overwrite one pointer, and removing the run removes what it built from.
pub const BUILT_REFERENCE_FILE: &str = "built-image";

/// What a build was asked to do, and what a run and `lns sandbox build` both hand the executor.
pub(crate) struct BuildRequest<'a> {
    pub args: &'a RunImageArgs,
    pub definition: &'a str,
    pub image: &'a str,
    pub rebuild: bool,
}

/// What one build produced: the image, the file it came from, and the key it is remembered under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuiltForRun {
    pub reference: String,
    pub label: String,
    pub layers: usize,
    pub key: String,
    /// True when the key answered outright, so this build ran nothing.
    pub reused: bool,
    pub reused_steps: usize,
}

pub(crate) fn names_a_containerfile(image: &str) -> bool {
    locate::names_a_containerfile(image)
}

/// Build what `spec.image` names, or hand back what this machine built for it already.
pub(crate) async fn build_for_run(
    run_id: &str,
    args: &RunImageArgs,
    definition: &str,
    image: &str,
    frame_tx: Sender<WireFrame>,
) -> Result<BuiltForRun> {
    let built = build(
        &BuildRequest {
            args,
            definition,
            image,
            rebuild: false,
        },
        frame_tx,
    )
    .await?;
    publish(&crate::cache::root()?, run_id, &built.reference);
    log::info!(
        "Image",
        "{}",
        step::built_line(&built.label, &built.reference)
    );
    Ok(built)
}

/// The build itself, with no run around it: `lns sandbox build` needs one, and a run is a caller.
pub(crate) async fn build(
    request: &BuildRequest<'_>,
    frame_tx: Sender<WireFrame>,
) -> Result<BuiltForRun> {
    let image = request.image;
    let definition_dir = request.args.definition_dir.as_deref().with_context(|| {
        format!(
            "spec.image {image:?} names a Containerfile beside the document, and this run carries no document directory to look in"
        )
    })?;
    let located = locate::locate(&RealContextFs, Path::new(definition_dir), image)?;
    let text = std::fs::read_to_string(&located.containerfile)
        .with_context(|| format!("reading {}", located.containerfile.display()))?;
    let file = super::parse::parse(&text).map_err(|refusals| {
        anyhow::anyhow!(
            "{} is not a Containerfile lns can build:\n  {}",
            located.label,
            refusals.join("\n  ")
        )
    })?;
    let context_hash = key::context_hash(&RealContextFs, &located.context)?;
    let policy = key::policy_fingerprint(request.definition)?;

    let cache_dir = crate::cache::root()?;
    let arch = crate::image::want_arch().to_string();
    let started = std::time::Instant::now();
    let host = RealBuildHost {
        context: located.context.clone(),
        source: located.containerfile.display().to_string(),
        args: request.args.clone(),
        definition: request.definition.to_string(),
        cache_dir: cache_dir.clone(),
        frame_tx,
        steps: AtomicUsize::new(0),
    };
    let plan = executor::BuildPlan {
        file: &file,
        label: &located.label,
        text: &text,
        context_hash: &context_hash,
        arch: &arch,
        rebuild: request.rebuild,
        policy: &policy,
    };
    let built = executor::build(&host, &plan)
        .await
        .with_context(|| format!("building {}", located.label))?;
    if !built.reused {
        log::info!(
            "Built",
            "{} in {:.2?} ({} layer{})",
            built.reference,
            started.elapsed(),
            built.layers,
            if built.layers == 1 { "" } else { "s" },
        );
    }
    Ok(BuiltForRun {
        reference: built.reference,
        label: located.label,
        layers: built.layers,
        key: built.key,
        reused: built.reused,
        reused_steps: built.reused_steps,
    })
}

/// What `lns sandbox build` was asked to build: the resolved document, and the policy a step is held to.
pub struct SandboxBuild<'a> {
    pub definition: &'a str,
    pub definition_dir: &'a str,
    pub rebuild: bool,
    pub authored_egress: Option<&'a str>,
    pub packed_filesets: &'a [lns_ipc::PackedFilesetSource],
}

/// `lns sandbox build`: the build a run would do, with no run around it and nothing published.
pub async fn build_sandbox(request: &SandboxBuild<'_>) -> Result<lns_ipc::Response> {
    let definition = request.definition;
    let image = image_of(definition)?;
    let (frame_tx, mut frames) = tokio::sync::mpsc::channel(1);
    // A build asked for on its own has no run to carry a step's output to, so it is drained here rather than left to fill.
    tokio::spawn(async move { while frames.recv().await.is_some() {} });
    let mut args = args_for_a_build(definition, request.definition_dir);
    args.image = Some(image.clone());
    args.authored_egress = request.authored_egress.map(str::to_string);
    args.packed_filesets = request.packed_filesets.to_vec();
    let built = build(
        &BuildRequest {
            args: &args,
            definition,
            image: &image,
            rebuild: request.rebuild,
        },
        frame_tx,
    )
    .await?;
    Ok(lns_ipc::Response::SandboxBuilt {
        key: built.key,
        reference: built.reference,
        label: built.label,
        layers: built.layers,
        reused: built.reused,
    })
}

/// What `spec.image` names, refused here when it names an image rather than a file to build.
fn image_of(definition: &str) -> Result<String> {
    let document: serde_json::Value =
        serde_json::from_str(definition).context("reading the document to build")?;
    let image = document["spec"]["image"]
        .as_str()
        .context("this document declares no spec.image, so there is nothing to build")?
        .to_string();
    if !names_a_containerfile(&image) {
        bail!(
            "spec.image {image:?} names an image to pull, not a Containerfile beside the document; there is nothing to build"
        );
    }
    Ok(image)
}

/// The run a build step is shaped from when no run asked for the build: the document, its directory, and this machine's defaults.
fn args_for_a_build(definition: &str, definition_dir: &str) -> RunImageArgs {
    RunImageArgs {
        image: None,
        resolved_image: None,
        mixins: Vec::new(),
        composed_mixins: Vec::new(),
        name: None,
        cpus: lns_artifact::resources::DEFAULT_VM_SIZE.cpus,
        mem: lns_artifact::resources::DEFAULT_VM_SIZE.mem_mib,
        cpus_explicit: false,
        mem_explicit: false,
        cpus_config: None,
        mem_config: None,
        sandbox_user: None,
        sandbox_uid: None,
        entrypoint: None,
        hostname: None,
        cmd: Vec::new(),
        env: Vec::new(),
        workdir: None,
        debug: false,
        tty: false,
        stdin: false,
        initial_winsize: None,
        detached: false,
        published_ports: Vec::new(),
        volumes: Vec::new(),
        binds: Vec::new(),
        auto_remove: false,
        verify_sandbox: false,
        definition: Some(definition.to_string()),
        definition_dir: Some(definition_dir.to_string()),
        authored_egress: None,
        packed_filesets: Vec::new(),
        denied_host_paths: Vec::new(),
    }
}

/// `lns sandbox prune`'s half of the build cache: the entries whose document has gone, and the
/// images those entries were the last to name.
pub struct RealBuiltImageSweep;

impl crate::ipc::BuiltImageSweep for RealBuiltImageSweep {
    async fn sweep(&self, cache_root: &Path, surviving_runs: &[String]) -> Result<Vec<String>> {
        let _exclusive = crate::image_store::cache_lock().write().await;
        let named = super::cache::still_referenced(
            &RealCacheFs,
            cache_root,
            surviving_runs,
            &|reference| holds_the_image(cache_root, reference),
        )
        .kept;
        crate::image_store::remove_unreferenced_builds_with(
            &crate::image_store::RealFs,
            &crate::image_store::real::RealCaches::new(cache_root),
            &cache_root.join("images"),
            &crate::image_store::recorded_run_pins().await?,
            &named,
        )
        .await
    }
}

/// What a build guest wrote, read off its upper volume once the guest has stopped.
pub(crate) fn capture_change_set(
    run_id: &str,
    guest_stop: crate::run::GuestStop,
) -> Result<ChangeSet> {
    super::refuse_unless_the_guest_stopped(guest_stop)?;
    let upper_image = crate::cache::run_dir(&crate::cache::root()?, run_id).join("upper.img");
    let tree = Ext4Upper::open_run_upper(&upper_image)?;
    upper::capture(&tree)
}

fn publish(cache_dir: &Path, run_id: &str, reference: &str) {
    let path = crate::cache::run_dir(cache_dir, run_id).join(BUILT_REFERENCE_FILE);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        log::warn!("the built image reference was not written: {e}");
        return;
    }
    if let Err(e) = std::fs::write(&path, format!("{reference}\n")) {
        log::warn!(
            "the built image reference was not written to {}: {e}",
            path.display()
        );
    }
}

struct RealBuildHost {
    context: PathBuf,
    /// The Containerfile this build reads, which every entry records so a sweep can ask whether the document is still here.
    source: String,
    args: RunImageArgs,
    definition: String,
    cache_dir: PathBuf,
    frame_tx: Sender<WireFrame>,
    steps: AtomicUsize,
}

impl BuildHost for RealBuildHost {
    async fn resolve_base(&self, image: &str) -> Result<Base> {
        let layers = LayerCache::new(self.cache_dir.join("layers"));
        let ingested = crate::ingest::run(
            Some(image),
            &[],
            &crate::image::want_arch(),
            &layers,
            crate::image::pull,
        )
        .await?;
        let reference = ingested
            .manifest_reference
            .with_context(|| format!("the base image {image} resolved to no manifest"))?;
        let parent = parent_image(&self.cache_dir.join("manifests"), &reference)?;
        Ok(Base {
            env: super::image::declared_env(&parent.config)?,
            reference,
        })
    }

    async fn run(&self, step: &RunStep) -> Result<RunOutcome> {
        let ordinal = self.steps.fetch_add(1, Ordering::SeqCst) + 1;
        let run_id = crate::run_registry::allocate_run_id();
        let args = step::args_for(&self.args, &self.definition, step)?;
        log::info!(
            "Running",
            "instruction {ordinal} on line {}: {}",
            step.line,
            step.argv.join(" "),
        );
        let outcome = crate::run::run_build_step(run_id, args, self.frame_tx.clone()).await?;
        if outcome.code != 0 {
            bail!(
                "the build guest exited {} running {}",
                outcome.code,
                step.argv.join(" ")
            );
        }
        Ok(RunOutcome {
            changes: outcome.changes,
            fileset_paths: outcome.fileset_paths,
        })
    }

    async fn holds_a_directory_at(&self, parent: &str, path: &str) -> Result<bool> {
        let manifests = self.cache_dir.join("manifests");
        let digests: Vec<String> = parent_image(&manifests, parent)?
            .manifest
            .layers
            .iter()
            .map(|layer| layer.digest.clone())
            .collect();
        let layers = CachedLayers(LayerCache::new(self.cache_dir.join("layers")));
        Ok(super::tree::entry_at(&layers, &digests, path)? == super::tree::ParentEntry::Directory)
    }

    async fn copy(&self, step: &CopyStep) -> Result<ChangeSet> {
        super::context::stage(&RealContextFs, &self.context, step)
    }

    async fn cached(&self, kind: Kind, key: &str) -> Option<String> {
        BuildCache::new(&RealCacheFs, &self.cache_dir)
            .get(kind, key, &|reference| self.holds(reference))
            .map(|entry| entry.reference)
    }

    async fn remember(&self, kind: Kind, key: &str, reference: &str) {
        if let Err(e) = BuildCache::new(&RealCacheFs, &self.cache_dir).remember(
            kind,
            key,
            &Entry {
                reference: reference.to_string(),
                source: self.source.clone(),
            },
        ) {
            log::warn!("this build will not be reused; its key was not written: {e:#}");
        }
    }

    async fn commit(&self, commit: &Commit<'_>) -> Result<String> {
        let manifests = self.cache_dir.join("manifests");
        let parent = parent_image(&manifests, commit.parent)?;
        let layers = LayerCache::new(self.cache_dir.join("layers"));
        let built = super::commit_step(
            &crate::image_store::RealFs,
            &LocalStore {
                layers: &layers,
                manifests,
                images: self.cache_dir.join("images"),
            },
            &super::StepCommit {
                changes: commit.layer,
                parent: &parent,
                draft: commit.config,
                created_by: commit.created_by,
                now_unix_secs: now_unix_secs(),
            },
        )
        .await?;
        if let Some(digest) = &built.layer_digest {
            log::debug!(
                digest = %digest,
                bytes = built.layer_bytes,
                entries = built.entries,
                "one instruction became one layer",
            );
        }
        Ok(built.reference)
    }
}

impl RealBuildHost {
    /// A key answers only while the image it names is still in this machine's manifest cache.
    fn holds(&self, reference: &str) -> bool {
        holds_the_image(&self.cache_dir, reference)
    }
}

/// Whether this machine still has the built image a key names.
pub(crate) fn holds_the_image(cache_dir: &Path, reference: &str) -> bool {
    ManifestCache::new(cache_dir.join("manifests"))
        .get(reference)
        .is_some()
}

/// The build cache directory as the host holds it.
pub(crate) struct RealCacheFs;

impl CacheFs for RealCacheFs {
    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
    }

    fn remove(&self, path: &Path) -> Result<()> {
        std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
    }

    fn list(&self, dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .collect()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// The image's own layer blobs, as the local layer cache holds them.
struct CachedLayers(LayerCache);

impl super::tree::LayerBytes for CachedLayers {
    fn read(&self, digest: &str) -> Result<Vec<u8>> {
        self.0.read(digest)
    }
}

fn parent_image(manifests: &Path, reference: &str) -> Result<ParentImage> {
    let normalized = crate::image_store::normalize_reference(reference)?;
    let cached = ManifestCache::new(manifests)
        .get(&normalized)
        .with_context(|| {
            format!(
                "the image {normalized} is not in the local manifest cache; \
                 a build stands on the manifest its own pull resolved"
            )
        })?;
    Ok(ParentImage {
        manifest: cached.manifest,
        config: cached.config,
    })
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The build context as it is on this host: a symlink is what it is, never what it points at.
struct RealContextFs;

impl ContextFs for RealContextFs {
    fn meta(&self, path: &Path) -> Result<Option<Meta>> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) => Ok(Some(Meta {
                kind: if meta.is_dir() {
                    EntryKind::Directory
                } else if meta.is_symlink() {
                    EntryKind::Symlink
                } else {
                    EntryKind::Regular
                },
                mode: mode_of(&meta),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn entries(&self, path: &Path) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in
            std::fs::read_dir(path).with_context(|| format!("listing {}", path.display()))?
        {
            names.push(
                entry
                    .with_context(|| format!("listing {}", path.display()))?
                    .file_name()
                    .to_string_lossy()
                    .to_string(),
            );
        }
        Ok(names)
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        std::fs::read(path).with_context(|| format!("reading {}", path.display()))
    }

    fn read_link(&self, path: &Path) -> Result<String> {
        Ok(std::fs::read_link(path)
            .with_context(|| format!("reading the symlink {}", path.display()))?
            .to_string_lossy()
            .to_string())
    }
}

#[cfg(unix)]
fn mode_of(meta: &std::fs::Metadata) -> u32 {
    std::os::unix::fs::MetadataExt::mode(meta) & 0o7777
}

#[cfg(not(unix))]
fn mode_of(_meta: &std::fs::Metadata) -> u32 {
    0o644
}
