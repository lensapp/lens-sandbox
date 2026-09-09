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

use super::context::{ContextFs, EntryKind, Meta};
use super::executor::{self, BuildHost, Commit, CopyStep, RunOutcome, RunStep};
use super::ext4_upper::Ext4Upper;
use super::image::ParentImage;
use super::import::LocalStore;
use super::locate::{self, Located};
use super::step;
use super::upper::{self, ChangeSet};

/// Where the reference of the image a run built lands, in the run's own directory: two runs ending together would overwrite one pointer, and removing the run removes what it built from.
pub const BUILT_REFERENCE_FILE: &str = "built-image";

/// Where "already built for this document on this machine" is remembered. Slice 4 of
/// lensapp/lens-sandbox#393 replaces this with the real key — the `FROM` digest, the context's
/// content hash and the architecture — and with `lns sandbox build`.
const BUILT_POINTER_DIR: &str = "builds";

/// What a run boots from when its `spec.image` named a Containerfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuiltForRun {
    pub reference: String,
    pub label: String,
    pub layers: usize,
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
    let definition_dir = args.definition_dir.as_deref().with_context(|| {
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

    let cache_dir = crate::cache::root()?;
    let pointer = pointer_path(&cache_dir, &located, &text);
    let built = match already_built(&cache_dir, &pointer) {
        Some(reference) => BuiltForRun {
            reference,
            label: located.label.clone(),
            layers: 0,
        },
        None => {
            let built =
                run_the_instructions(args, definition, &located, &file, &cache_dir, frame_tx)
                    .await?;
            remember(&cache_dir, &pointer, &built.reference);
            built
        }
    };
    publish(&cache_dir, run_id, &built.reference);
    log::info!(
        "Image",
        "{}",
        step::built_line(&built.label, &built.reference)
    );
    Ok(built)
}

async fn run_the_instructions(
    args: &RunImageArgs,
    definition: &str,
    located: &Located,
    file: &super::parse::Containerfile,
    cache_dir: &Path,
    frame_tx: Sender<WireFrame>,
) -> Result<BuiltForRun> {
    let started = std::time::Instant::now();
    log::info!(
        "Building",
        "{} ({} instructions)",
        located.label,
        file.instructions.len()
    );
    let host = RealBuildHost {
        context: located.context.clone(),
        args: args.clone(),
        definition: definition.to_string(),
        cache_dir: cache_dir.to_path_buf(),
        frame_tx,
        steps: AtomicUsize::new(0),
    };
    let built = executor::build(&host, file)
        .await
        .with_context(|| format!("building {}", located.label))?;
    log::info!(
        "Built",
        "{} in {:.2?} ({} layer{})",
        built.reference,
        started.elapsed(),
        built.layers,
        if built.layers == 1 { "" } else { "s" },
    );
    Ok(BuiltForRun {
        reference: built.reference,
        label: located.label.clone(),
        layers: built.layers,
    })
}

/// "Already built for this document on this machine", which is all the cache slice 3 keeps.
fn remember(cache_dir: &Path, pointer: &Path, reference: &str) {
    if let Err(e) = std::fs::create_dir_all(pointer.parent().unwrap_or(cache_dir))
        .and_then(|()| std::fs::write(pointer, format!("{reference}\n")))
    {
        log::warn!("this build will not be reused; its pointer was not written: {e}");
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

fn already_built(cache_dir: &Path, pointer: &Path) -> Option<String> {
    let reference = std::fs::read_to_string(pointer).ok()?.trim().to_string();
    ManifestCache::new(cache_dir.join("manifests"))
        .get(&reference)
        .map(|_| reference)
}

/// One document built on one machine: the file's own text and where it sits, plus the architecture a guest boots.
fn pointer_path(cache_dir: &Path, located: &Located, text: &str) -> PathBuf {
    let key = <sha2::Sha256 as sha2::Digest>::digest(
        format!(
            "{}\0{}\0{text}",
            located.containerfile.display(),
            crate::image::want_arch(),
        )
        .as_bytes(),
    );
    cache_dir.join(BUILT_POINTER_DIR).join(hex::encode(key))
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
    args: RunImageArgs,
    definition: String,
    cache_dir: PathBuf,
    frame_tx: Sender<WireFrame>,
    steps: AtomicUsize,
}

impl BuildHost for RealBuildHost {
    async fn resolve_base(&self, image: &str) -> Result<String> {
        let layers = LayerCache::new(self.cache_dir.join("layers"));
        let ingested = crate::ingest::run(
            Some(image),
            &[],
            &crate::image::want_arch(),
            &layers,
            crate::image::pull,
        )
        .await?;
        ingested
            .manifest_reference
            .with_context(|| format!("the base image {image} resolved to no manifest"))
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

    async fn copy(&self, step: &CopyStep) -> Result<ChangeSet> {
        super::context::stage(&RealContextFs, &self.context, step)
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
                created: &crate::time_fmt::rfc3339_now(),
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
