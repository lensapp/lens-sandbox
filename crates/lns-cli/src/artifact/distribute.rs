use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use lns_artifact::build::BuiltArtifact;

use super::author::Fs;
use crate::local_future::LocalBoxFuture;

/// Uploads a built OCI artifact — its config blob, every packed fileset layer, then the manifest; the real impl reuses the `lns login` credential, a fake drives the push scenarios offline.
pub trait Producer {
    fn push_built<'a>(
        &'a self,
        built: &'a BuiltArtifact,
        reference: &'a str,
    ) -> LocalBoxFuture<'a, Result<()>>;

    /// Upload a built image's config, layers and manifest into the artifact's own repository, under the tag this architecture owns (§6).
    fn push_image<'a>(
        &'a self,
        image: &'a lns_ipc::PushableImage,
        repository: &'a str,
        tag: &'a str,
    ) -> LocalBoxFuture<'a, Result<()>>;

    /// What one architecture's image tag holds today, so a push assembles the index over every architecture the repository already publishes.
    fn image_at<'a>(
        &'a self,
        repository: &'a str,
        tag: &'a str,
    ) -> LocalBoxFuture<'a, Result<Option<lns_artifact::image_index::IndexEntry>>>;

    /// The index this reference's document names today, so a push adds its entry to what that index holds and still repairs one another push left behind (§6.2).
    fn index_at<'a>(
        &'a self,
        repository: &'a str,
        tag: &'a str,
    ) -> LocalBoxFuture<'a, Result<Option<lns_artifact::image_index::HeldIndex>>>;

    /// Upload the assembled image index, which is what the published document names by digest.
    fn push_index<'a>(
        &'a self,
        repository: &'a str,
        tag: &'a str,
        index: &'a [u8],
    ) -> LocalBoxFuture<'a, Result<()>>;
}

/// Builds the image a path-form `spec.image` names. The service owns the executor and the layer store; the CLI owns the registry login, so the image is built there and uploaded here.
pub trait ImageBuilder {
    /// Merge the document's mixins, so a build step reaches the plan as resolved as the run of the same document would be.
    fn resolve<'a>(
        &'a self,
        document: &'a [u8],
        project_dir: &'a Path,
    ) -> LocalBoxFuture<'a, Result<ResolvedDefinition>>;

    fn build<'a>(&'a self, request: &'a ImageRequest<'a>)
    -> LocalBoxFuture<'a, Result<BuiltImage>>;
}

/// What one push asks the builder for.
pub struct ImageRequest<'a> {
    /// The document as canonical JSON, resolved, which carries the path `spec.image` names.
    pub document: Vec<u8>,
    /// The document's directory, which roots that path.
    pub project_dir: &'a Path,
    /// Ignore every key this build would otherwise answer from (`--rebuild`).
    pub rebuild: bool,
    /// Answer with the key alone and build nothing (`--dry-run`).
    pub plan_only: bool,
    /// The egress the document's other sources authored, so a build step reaches only what a run of the same document would.
    pub authored_egress: Option<String>,
    /// Which artifact carries each packed fileset the resolution reached, so a build step seeds the same files a run would.
    pub packed_filesets: Vec<lns_ipc::PackedFilesetSource>,
}

/// What the service answered when it merged a document's mixins.
pub struct ResolvedDefinition {
    pub definition: Vec<u8>,
    pub authored_egress: Option<String>,
    pub packed_filesets: Vec<lns_ipc::PackedFilesetSource>,
}

/// What the builder answered: the key that names the build, and the image itself when this machine has one.
pub struct BuiltImage {
    pub key: String,
    pub label: String,
    pub reused: bool,
    /// Absent only from a plan whose key this machine cannot answer.
    pub image: Option<lns_ipc::PushableImage>,
}

/// What a push published for a path-form `spec.image`: the digest the document now names, and the key that names the build behind it.
pub struct PublishedImage {
    pub key: String,
    pub label: String,
    pub reference: String,
}

/// Resolves a declared tool's (possibly fuzzy) version to the exact version the published artifact pins, by consulting the tool's public version index; a fake scripts it offline.
pub trait ToolResolver {
    fn resolve<'a>(
        &'a self,
        tool: &'a lns_artifact::tools::ToolRef,
    ) -> LocalBoxFuture<'a, Result<String>>;

    fn verify<'a>(
        &'a self,
        tool: &'a lns_artifact::tools::ToolRef,
    ) -> LocalBoxFuture<'a, IndexVerification>;
}

/// What the index said about an already-exact pin: best-effort only, because a required answer would break offline re-push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexVerification {
    Confirmed,
    Absent,
    Unavailable,
}

/// What a declared entry was published as, so the publisher sees the version they shipped rather than having to read it back out of the registry.
#[derive(Debug)]
pub struct PinnedTool {
    pub declared: String,
    pub published: String,
    pub verification: Option<IndexVerification>,
}

/// Rewrite `spec.tools` so every entry carries the exact version the index resolves today — the tool analogue of digest-pinning path filesets at push.
pub async fn pin_declared_tools<R: ToolResolver + ?Sized>(
    resolver: &R,
    doc: &[u8],
) -> Result<(Vec<u8>, Vec<PinnedTool>)> {
    let mut value: serde_json::Value =
        serde_json::from_slice(doc).context("re-reading the definition for tool pinning")?;
    let Some(entries) = value["spec"]["tools"].as_array_mut() else {
        return Ok((doc.to_vec(), Vec::new()));
    };
    if entries.is_empty() {
        return Ok((doc.to_vec(), Vec::new()));
    }
    let mut pinned = Vec::with_capacity(entries.len());
    for entry in entries {
        let declared = entry.as_str().context("spec.tools entry is not a string")?;
        let tool = lns_artifact::tools::parse(declared)?;
        // An entry that already names an exact version is its own pin; re-publishing must not need the index, which may be blocked or may have dropped that version — so its verification is best-effort, never a veto.
        let (published, verification) = if lns_artifact::tools::is_exact_version(&tool.version) {
            (declared.to_string(), Some(resolver.verify(&tool).await))
        } else {
            let exact = resolver
                .resolve(&tool)
                .await
                .with_context(|| format!("resolving {declared} for publishing"))?;
            (format!("{}@{exact}", tool.name), None)
        };
        pinned.push(PinnedTool {
            declared: declared.to_string(),
            published: published.clone(),
            verification,
        });
        *entry = serde_json::Value::String(published);
    }
    let doc = serde_json::to_vec(&value).context("serializing the tool-pinned definition")?;
    Ok((doc, pinned))
}

/// The directory one `filesets[].path` entry ships, read off the author's machine ready to pack.
#[derive(Debug)]
pub struct PackedDirectory {
    pub path: String,
    pub files: Vec<lns_artifact::build::FileEntry>,
}

/// Read every `path` fileset the document declares, in declaration order, so each becomes a layer of the artifact this document configures (`docs/sandbox-spec.md` §6). The entry itself publishes untouched: it keeps its `path`, `guestPath` and `owner`.
pub fn pack_path_filesets<F: Fs + ?Sized>(
    fs: &F,
    cwd: &Path,
    doc: &[u8],
) -> Result<Vec<PackedDirectory>> {
    let kind = lns_artifact::spec::read_kind(doc)
        .map_err(|e| anyhow::anyhow!("refusing to push an invalid document: {e:#}"))?;
    let def = lns_artifact::sandbox::parse_document(doc)
        .map_err(|e| anyhow::anyhow!("refusing to push an invalid document: {e:#}"))?;
    def.path_filesets()
        .into_iter()
        .map(|path| {
            let files = super::fileset::walk(fs, &cwd.join(path), kind)
                .with_context(|| format!("fileset {path}"))?;
            Ok(PackedDirectory {
                path: path.to_string(),
                files,
            })
        })
        .collect()
}

/// The `README.md` beside the document, refused when it is a symlink or over-limit — it publishes automatically, so it gets the same boundary a packed fileset draws, and it is checked before any upload.
fn read_readme<F: Fs + ?Sized>(fs: &F, cwd: &Path) -> Result<Option<Vec<u8>>> {
    let path = cwd.join("README.md");
    if fs.is_symlink(&path) {
        bail!(
            "README.md is a symlink — a README publishes automatically, so it must be a regular file in the project"
        );
    }
    if !fs.exists(&path) {
        return Ok(None);
    }
    let bytes = fs
        .read_limited(&path, lns_artifact::build::MAX_README_BYTES)
        .with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() as u64 > lns_artifact::build::MAX_README_BYTES {
        bail!(
            "README.md exceeds the {}-byte limit",
            lns_artifact::build::MAX_README_BYTES
        );
    }
    Ok(Some(bytes))
}

/// Validate every README a push would publish — the document's and each planned mixin's — so a refusal fires before the first upload and leaves nothing published.
fn preflight_readmes<F: Fs + ?Sized>(
    fs: &F,
    cwd: &Path,
    plan: &super::mixin_plan::MixinPlan,
) -> Result<()> {
    read_readme(fs, cwd)?;
    for node in &plan.nodes {
        read_readme(fs, &node.root).with_context(|| format!("mixin {}", node.declared))?;
    }
    Ok(())
}

/// Build the artifact a push uploads: the document, one layer per `path` fileset it declares, the README beside it, and — when `spec.image` named a Containerfile — that file with its context (§7.3).
fn build<F: Fs + ?Sized>(
    fs: &F,
    cwd: &Path,
    doc: &[u8],
    source: Option<&lns_artifact::build_source::ImageSourceLayer>,
) -> Result<(BuiltArtifact, Vec<PackedDirectory>)> {
    let packed = pack_path_filesets(fs, cwd, doc)?;
    let layers: Vec<Vec<lns_artifact::build::FileEntry>> =
        packed.iter().map(|dir| dir.files.clone()).collect();
    let readme = read_readme(fs, cwd)?;
    let built = lns_artifact::build::build_artifact_with(doc, &layers, readme.as_deref(), source)?;
    Ok((built, packed))
}

/// What the publisher sees for the Containerfile that became a layer: the file they named, and the digest its content published under (§7.3).
fn report_build_source<W: Write>(
    out: &mut W,
    verb: &str,
    source: Option<&lns_artifact::build_source::ImageSourceLayer>,
    built: &BuiltArtifact,
) -> Result<()> {
    if let (Some(source), Some(layer)) = (source, built.build_source_layer()) {
        writeln!(
            out,
            "{verb} {} -> {} ({} bytes)",
            source.containerfile(),
            layer.digest,
            layer.data.len()
        )?;
    }
    Ok(())
}

/// What the publisher sees for each directory that became a layer: the entry they wrote, and the digest its content published under.
fn report_packed<W: Write>(
    out: &mut W,
    verb: &str,
    built: &BuiltArtifact,
    packed: &[PackedDirectory],
) -> Result<()> {
    for (dir, layer) in packed.iter().zip(built.fileset_layers()) {
        writeln!(
            out,
            "{verb} fileset {} -> {} ({} bytes)",
            dir.path,
            layer.digest,
            layer.data.len()
        )?;
    }
    if let Some(layer) = built.readme_layer() {
        writeln!(
            out,
            "{verb} README.md -> {} ({} bytes)",
            layer.digest,
            layer.data.len()
        )?;
    }
    Ok(())
}

/// What a dry run needs of the world: the same document sources a push reads, and the same size discipline it is held to.
pub struct DryRunPorts<'a, F: Fs + ?Sized, P: Producer + ?Sized, B: ImageBuilder + ?Sized> {
    pub fs: &'a F,
    pub cwd: &'a Path,
    /// A dry run uploads nothing and reads the registry, so it can say which architectures the index it would name already holds.
    pub producer: &'a P,
    pub builder: &'a B,
    pub image_limit: u64,
    pub rebuild: bool,
}

/// What one push drives: the author's files, the directory that roots them, the registry, the version index, the builder behind a path-form `spec.image`, and what this machine lets a built image weigh.
pub struct PushPorts<
    'a,
    F: Fs + ?Sized,
    P: Producer + ?Sized,
    R: ToolResolver + ?Sized,
    B: ImageBuilder + ?Sized,
> {
    pub fs: &'a F,
    pub cwd: &'a Path,
    pub producer: &'a P,
    pub resolver: &'a R,
    pub builder: &'a B,
    pub image_limit: u64,
    pub rebuild: bool,
}

/// How a push may accept publishing the local mixins a document names.
pub struct Confirm<'a> {
    pub assume_yes: bool,
    pub terminal: &'a mut dyn crate::terminal::Terminal,
}

/// What each local mixin published as, keyed by its document so every entry naming it pins the same digest.
type PublishedMixins = Vec<(std::path::PathBuf, String)>;

/// Publish each planned mixin under its own repository, children first, so every digest exists before the document that pins it is built.
async fn publish_planned_mixins<F, P, R, W>(
    fs: &F,
    producer: &P,
    resolver: &R,
    plan: &super::mixin_plan::MixinPlan,
    out: &mut W,
) -> Result<PublishedMixins>
where
    F: Fs + ?Sized,
    P: Producer + ?Sized,
    R: ToolResolver + ?Sized,
    W: Write,
{
    let mut published: PublishedMixins = Vec::new();
    for node in &plan.nodes {
        let doc = super::mixin_plan::pin_local_mixins(fs, &node.root, &node.bytes, &published)?;
        let (doc, _) = pin_declared_tools(resolver, &doc).await?;
        let (built, packed) = build(fs, &node.root, &doc, None)?;
        report_packed(out, "packed", &built, &packed)?;
        let tag = super::mixin_plan::digest_derived_tag(&built.manifest_digest);
        producer
            .push_built(&built, &format!("{}:{tag}", node.repository))
            .await
            .with_context(|| {
                format!(
                    "publishing mixin {}; the sandbox was not published, and re-running the push re-derives the same digests, so retrying is safe",
                    node.declared
                )
            })?;
        writeln!(
            out,
            "published mixin {} → {}@{}",
            node.declared, node.repository, built.manifest_digest
        )?;
        published.push((
            node.document.clone(),
            format!("{}@{}", node.repository, built.manifest_digest),
        ));
    }
    Ok(published)
}

/// The same walk offline: each digest is computable without the network, so the preview names what every artifact would publish as.
fn preview_planned_mixins<F, W>(
    fs: &F,
    plan: &super::mixin_plan::MixinPlan,
    out: &mut W,
) -> Result<PublishedMixins>
where
    F: Fs + ?Sized,
    W: Write,
{
    let mut published: PublishedMixins = Vec::new();
    for node in &plan.nodes {
        let doc = super::mixin_plan::pin_local_mixins(fs, &node.root, &node.bytes, &published)?;
        let (built, packed) = build(fs, &node.root, &doc, None)?;
        report_packed(out, "would pack", &built, &packed)?;
        writeln!(
            out,
            "would publish mixin {} → {}@{} ({})",
            node.declared,
            node.repository,
            built.manifest_digest,
            blob_bytes(&built)
        )?;
        published.push((
            node.document.clone(),
            format!("{}@{}", node.repository, built.manifest_digest),
        ));
    }
    Ok(published)
}

/// A mixin rides along as its own artifact, so it faces the same gate as the document that names it — otherwise a tool no consumer can provision publishes just by being layered on.
fn refuse_unpushable_planned_tools(plan: &super::mixin_plan::MixinPlan) -> Result<()> {
    for node in &plan.nodes {
        refuse_unpushable_tools(&node.bytes).with_context(|| format!("mixin {}", node.declared))?;
    }
    Ok(())
}

fn refuse_unpushable_tools(doc: &[u8]) -> Result<()> {
    lns_artifact::validate::refuse_unprovisionable_tools(doc).map_err(|problem| {
        anyhow::anyhow!("refusing to push a sandbox no consumer can start: {problem}")
    })
}

/// What `spec.image` names, read straight out of the document a push was handed.
fn declared_image(doc: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(doc)
        .ok()
        .and_then(|doc| doc["spec"]["image"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Whether this document's image is a Containerfile a push has to build before it can publish anything.
fn names_a_containerfile(doc: &[u8]) -> bool {
    matches!(
        lns_artifact::image::source(&declared_image(doc)),
        lns_artifact::image::ImageSource::Containerfile(_)
    )
}

/// A build step is the document's own run with one instruction in its place, so a push takes the same preflight a run takes: a document still declaring mixins reaches no plan.
async fn resolve_before_building<B>(builder: &B, request: &mut ImageRequest<'_>) -> Result<()>
where
    B: ImageBuilder + ?Sized,
{
    if !crate::resolve::declares_a_mixin(&request.document) {
        return Ok(());
    }
    let resolved = builder
        .resolve(&request.document, request.project_dir)
        .await?;
    request.document = resolved.definition;
    request.authored_egress = resolved.authored_egress;
    request.packed_filesets = resolved.packed_filesets;
    Ok(())
}

/// The entry this push contributes to the index: the manifest it just built, as an index addresses it.
fn entry_of(image: &lns_ipc::PushableImage) -> lns_artifact::image_index::IndexEntry {
    lns_artifact::image_index::IndexEntry {
        digest: image.digest.clone(),
        size: image.manifest.len() as u64,
        media_type: image.manifest_media_type.clone(),
        os: image.os.clone(),
        architecture: image.architecture.clone(),
    }
}

/// What this reference already publishes: the entries the index tag holds today, then the per-architecture tags, which are the record this lns keeps and so decide their own platforms. An entry for a platform this lns does not build survives, because a newer lns published it and an older one must not drop it (§6.2).
async fn held_entries<P>(
    producer: &P,
    repository: &str,
    artifact_tag: &str,
    published: Option<&lns_artifact::image_index::HeldIndex>,
) -> Result<Vec<lns_artifact::image_index::IndexEntry>>
where
    P: Producer + ?Sized,
{
    let mut held = published
        .map(|index| index.entries.clone())
        .unwrap_or_default();
    for architecture in lns_artifact::image_index::ARCHITECTURES {
        let tag = lns_artifact::image_index::architecture_tag(
            artifact_tag,
            lns_artifact::image_index::OS,
            architecture,
        );
        let entry = producer
            .image_at(repository, &tag)
            .await
            .with_context(|| format!("reading what {repository}:{tag} holds"))?;
        if let Some(entry) = entry {
            held = lns_artifact::image_index::with_entry(&held, entry);
        }
    }
    Ok(lns_artifact::image_index::in_index_order(held))
}

/// How every line that lists an index names what it holds.
fn holdings(entries: &[lns_artifact::image_index::IndexEntry]) -> String {
    entries
        .iter()
        .map(|entry| format!("{} {}", entry.platform(), entry.digest))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Build the image `spec.image` names and publish it into the artifact's own repository, before the document that will name its digest is uploaded — `lns push` never publishes a document whose image it does not have (§6). The document names an index, so a push from a second architecture adds an entry rather than replacing what the first published.
async fn publish_the_image<P, B, W>(
    producer: &P,
    builder: &B,
    request: &mut ImageRequest<'_>,
    reference: &str,
    limit: u64,
    out: &mut W,
) -> Result<Option<PublishedImage>>
where
    P: Producer + ?Sized,
    B: ImageBuilder + ?Sized,
    W: Write,
{
    if !names_a_containerfile(&request.document) {
        return Ok(None);
    }
    resolve_before_building(builder, request).await?;
    let built = builder.build(request).await?;
    writeln!(out, "key {}", built.key)?;
    let image = built
        .image
        .context("the build answered with no image, so there is nothing to publish")?;
    refuse_an_image_over(limit, &image)?;
    let repository = super::mixin_plan::repository_of(reference);
    let artifact_tag = lns_artifact::image_index::tag_of(reference);
    let index_tag = lns_artifact::image_index::index_tag(artifact_tag);
    let names_today = producer.index_at(repository, &index_tag).await?;
    let held = held_entries(producer, repository, artifact_tag, names_today.as_ref()).await?;
    let mine = entry_of(&image);
    let entries = lns_artifact::image_index::with_entry(&held, mine.clone());
    let index = lns_artifact::image_index::assemble(&entries)?;
    let published = format!("{repository}@{}", index.digest);
    if held == entries {
        writeln!(
            out,
            "nothing to publish: the index already holds {} {} for {}",
            mine.platform(),
            mine.digest,
            built.label
        )?;
    } else {
        publish_one_architecture(producer, &image, repository, artifact_tag, &mine).await?;
        writeln!(
            out,
            "{} {} as {repository}@{} ({} layer{})",
            if built.reused { "reused" } else { "built" },
            built.label,
            image.digest,
            image.layers.len(),
            if image.layers.len() == 1 { "" } else { "s" },
        )?;
    }
    // The entries are the record and the index is derived from them, so an index that is not the one they assemble to is republished even where this push added no entry.
    if names_today.map(|held| held.digest).as_ref() != Some(&index.digest) {
        producer
            .push_index(repository, &index_tag, &index.bytes)
            .await
            .with_context(|| format!("publishing the image index into {repository}"))?;
    }
    writeln!(out, "index {published} holds {}", holdings(&entries))?;
    Ok(Some(PublishedImage {
        key: built.key,
        label: built.label,
        reference: published,
    }))
}

/// This architecture's manifest goes up before the index that names it, and under the tag only this architecture writes.
async fn publish_one_architecture<P>(
    producer: &P,
    image: &lns_ipc::PushableImage,
    repository: &str,
    artifact_tag: &str,
    mine: &lns_artifact::image_index::IndexEntry,
) -> Result<()>
where
    P: Producer + ?Sized,
{
    let tag =
        lns_artifact::image_index::architecture_tag(artifact_tag, &mine.os, &mine.architecture);
    producer
        .push_image(image, repository, &tag)
        .await
        .with_context(|| format!("publishing the built image into {repository}"))
}

/// The size discipline (§6), read off the manifest the build produced.
fn refuse_an_image_over(limit: u64, image: &lns_ipc::PushableImage) -> Result<()> {
    let layers: Vec<lns_artifact::image::ImageLayer> = image
        .layers
        .iter()
        .map(|layer| lns_artifact::image::ImageLayer {
            digest: layer.digest.clone(),
            bytes: layer.size,
        })
        .collect();
    lns_artifact::image::refuse_an_image_over(limit, &layers, &image.config)
}

/// What a dry run can say about the image without building it: the key always, which architectures the index holds, and the digest only when this machine already answers that key.
async fn preview_the_image<P, B, W>(
    producer: &P,
    builder: &B,
    request: &mut ImageRequest<'_>,
    reference: &str,
    limit: u64,
    out: &mut W,
) -> Result<Option<String>>
where
    P: Producer + ?Sized,
    B: ImageBuilder + ?Sized,
    W: Write,
{
    if !names_a_containerfile(&request.document) {
        return Ok(None);
    }
    resolve_before_building(builder, request).await?;
    let planned = builder.build(request).await?;
    writeln!(out, "key {}", planned.key)?;
    let repository = super::mixin_plan::repository_of(reference);
    let artifact_tag = lns_artifact::image_index::tag_of(reference);
    let index_tag = lns_artifact::image_index::index_tag(artifact_tag);
    let names_today = producer.index_at(repository, &index_tag).await?;
    let held = held_entries(producer, repository, artifact_tag, names_today.as_ref()).await?;
    writeln!(
        out,
        "the index holds {}",
        match held.is_empty() {
            true => "nothing yet".to_string(),
            false => holdings(&held),
        }
    )?;
    match planned.image {
        Some(image) => {
            refuse_an_image_over(limit, &image)?;
            writeln!(
                out,
                "would publish the image {} builds to, {}",
                planned.label, image.digest
            )?;
            let mine = entry_of(&image);
            let entries = lns_artifact::image_index::with_entry(&held, mine.clone());
            if held == entries {
                writeln!(
                    out,
                    "this push would add nothing: the index already holds {} {}",
                    mine.platform(),
                    mine.digest
                )?;
            } else {
                writeln!(
                    out,
                    "this push would add {} {}",
                    mine.platform(),
                    mine.digest
                )?;
            }
            Ok(Some(lns_artifact::image_index::assemble(&entries)?.digest))
        }
        None => {
            writeln!(
                out,
                "note: {} has not been built on this machine, so its image digest can only be known by building; the published digest will differ from this preview",
                planned.label
            )?;
            Ok(None)
        }
    }
}

/// `lns push <ref>`: validate the document, pack each of its path filesets into a layer of the same artifact, and upload the whole thing in one step. The caller reads `./lns.yaml` into `doc`.
pub async fn push<F, P, R, B, W>(
    ports: PushPorts<'_, F, P, R, B>,
    doc: &[u8],
    reference: &str,
    confirm: Confirm<'_>,
    out: &mut W,
) -> Result<i32>
where
    F: Fs + ?Sized,
    P: Producer + ?Sized,
    R: ToolResolver + ?Sized,
    B: ImageBuilder + ?Sized,
    W: Write,
{
    let PushPorts {
        fs,
        cwd,
        producer,
        resolver,
        builder,
        image_limit,
        rebuild,
    } = ports;
    let Confirm {
        assume_yes,
        terminal,
    } = confirm;
    refuse_unpushable_tools(doc)?;
    // Packing first reads the directories offline, so a broken document or fileset refuses the push before it consults the index or uploads a mixin.
    pack_path_filesets(fs, cwd, doc)?;
    let source = super::image_build::source_layer(fs, cwd, &declared_image(doc))?;
    let plan = super::mixin_plan::plan_local_mixins(fs, cwd, doc, reference)?;
    refuse_unpushable_planned_tools(&plan)?;
    preflight_readmes(fs, cwd, &plan)?;
    super::mixin_plan::confirm_mixin_publication(&plan, reference, assume_yes, terminal, out)?;
    let image = publish_the_image(
        producer,
        builder,
        &mut ImageRequest {
            document: doc.to_vec(),
            project_dir: cwd,
            rebuild,
            plan_only: false,
            authored_egress: None,
            packed_filesets: Vec::new(),
        },
        reference,
        image_limit,
        out,
    )
    .await?;
    let published = publish_planned_mixins(fs, producer, resolver, &plan, out).await?;
    let doc = super::mixin_plan::pin_local_mixins(fs, cwd, doc, &published)?;
    let (doc, pinned_tools) = pin_declared_tools(resolver, &doc).await?;
    let doc = match &image {
        Some(image) => lns_artifact::image::rewrite_to_built(&doc, &image.reference)?,
        None => lns_artifact::image::forget_source(&doc)?,
    };
    let (built, packed) = build(fs, cwd, &doc, source.as_ref())?;
    report_packed(out, "packed", &built, &packed)?;
    report_build_source(out, "packed", source.as_ref(), &built)?;
    for tool in &pinned_tools {
        if tool.verification == Some(IndexVerification::Absent) {
            writeln!(
                out,
                "warning: the version index does not list {} today; consumers will provision it exactly as declared",
                tool.published
            )?;
        }
        writeln!(out, "pinned {} → {}", tool.declared, tool.published)?;
    }
    producer
        .push_built(&built, reference)
        .await
        .map_err(|e| match plan.nodes.len() {
            0 => e,
            published => e.context(format!(
                "the sandbox was not published; its {published} mixin(s) are already uploaded under their own digests, and re-running the push re-derives the same digests, so retrying is safe"
            )),
        })?;
    writeln!(
        out,
        "built and pushed {reference}@{}",
        built.manifest_digest
    )?;
    Ok(0)
}

/// `lns push --dry-run <ref>`: everything a push validates, packs, and builds — printing the digests that would publish; nothing is built and nothing is uploaded.
pub async fn push_dry_run<F, P, B, W>(
    ports: DryRunPorts<'_, F, P, B>,
    doc: &[u8],
    reference: &str,
    out: &mut W,
) -> Result<i32>
where
    F: Fs + ?Sized,
    P: Producer + ?Sized,
    B: ImageBuilder + ?Sized,
    W: Write,
{
    let DryRunPorts {
        fs,
        cwd,
        producer,
        builder,
        image_limit,
        rebuild,
    } = ports;
    refuse_unpushable_tools(doc)?;
    pack_path_filesets(fs, cwd, doc)?;
    let source = super::image_build::source_layer(fs, cwd, &declared_image(doc))?;
    let plan = super::mixin_plan::plan_local_mixins(fs, cwd, doc, reference)?;
    refuse_unpushable_planned_tools(&plan)?;
    preflight_readmes(fs, cwd, &plan)?;
    let digest = preview_the_image(
        producer,
        builder,
        &mut ImageRequest {
            document: doc.to_vec(),
            project_dir: cwd,
            rebuild,
            plan_only: true,
            authored_egress: None,
            packed_filesets: Vec::new(),
        },
        reference,
        image_limit,
        out,
    )
    .await?;
    let published = preview_planned_mixins(fs, &plan, out)?;
    let pinned = super::mixin_plan::pin_local_mixins(fs, cwd, doc, &published)?;
    let pinned = match &digest {
        Some(digest) => lns_artifact::image::rewrite_to_built(
            &pinned,
            &format!("{}@{digest}", super::mixin_plan::repository_of(reference)),
        )?,
        None => lns_artifact::image::forget_source(&pinned)?,
    };
    let (built, packed) = build(fs, cwd, &pinned, source.as_ref())?;
    report_packed(out, "would pack", &built, &packed)?;
    report_build_source(out, "would pack", source.as_ref(), &built)?;
    let mut docs: Vec<&[u8]> = vec![&pinned];
    docs.extend(plan.nodes.iter().map(|node| node.bytes.as_slice()));
    let unresolved_tools = unresolved_tool_count(&docs);
    if unresolved_tools > 0 {
        writeln!(
            out,
            "note: {unresolved_tools} tool version(s) resolve at push time; the published digest may differ from this preview"
        )?;
    }
    let bytes = blob_bytes(&built);
    writeln!(
        out,
        "would push {reference}@{} ({bytes})",
        built.manifest_digest
    )?;
    writeln!(out, "dry run — built and validated; nothing uploaded")?;
    Ok(0)
}

/// Only a fuzzy entry can change the published bytes; an exact pin is short-circuited, so counting it would promise a difference that cannot happen.
fn unresolved_tool_count(docs: &[&[u8]]) -> usize {
    docs.iter()
        .filter_map(|doc| serde_json::from_slice::<serde_json::Value>(doc).ok())
        .filter_map(|value| value["spec"]["tools"].as_array().cloned())
        .flatten()
        .filter_map(|entry| {
            entry
                .as_str()
                .and_then(|entry| lns_artifact::tools::parse(entry).ok())
        })
        .filter(|tool| !lns_artifact::tools::is_exact_version(&tool.version))
        .count()
}

fn blob_bytes(built: &BuiltArtifact) -> String {
    let bytes: usize = built.blobs.iter().map(|blob| blob.data.len()).sum();
    format!("{bytes} bytes")
}

#[cfg(test)]
mod tests {
    /// Every scenario below predates local-mixin publication, so its plan is empty and the prompt never fires; this keeps the call sites reading as the push they are about.
    async fn push_no_prompt<F, P, R, W>(
        fs: &F,
        cwd: &Path,
        producer: &P,
        resolver: &R,
        doc: &[u8],
        reference: &str,
        out: &mut W,
    ) -> Result<i32>
    where
        F: Fs + ?Sized,
        P: Producer + ?Sized,
        R: ToolResolver + ?Sized,
        W: Write,
    {
        push_with_builder(
            fs,
            cwd,
            producer,
            resolver,
            &FakeBuilder::unconsultable(),
            doc,
            reference,
            out,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)] // the ports one push drives, spelled out for a test that names one of them
    async fn push_with_builder<F, P, R, B, W>(
        fs: &F,
        cwd: &Path,
        producer: &P,
        resolver: &R,
        builder: &B,
        doc: &[u8],
        reference: &str,
        out: &mut W,
    ) -> Result<i32>
    where
        F: Fs + ?Sized,
        P: Producer + ?Sized,
        R: ToolResolver + ?Sized,
        B: ImageBuilder + ?Sized,
        W: Write,
    {
        push(
            PushPorts {
                fs,
                cwd,
                producer,
                resolver,
                builder,
                image_limit: lns_artifact::image::DEFAULT_IMAGE_LIMIT_BYTES,
                rebuild: false,
            },
            doc,
            reference,
            Confirm {
                assume_yes: false,
                terminal: &mut crate::terminal::ScriptedTerminal::answering(&["y\n"]),
            },
            out,
        )
        .await
    }

    /// A dry run of a document whose image is a reference never reaches the builder.
    async fn dry_run<F, W>(
        fs: &F,
        cwd: &Path,
        doc: &[u8],
        reference: &str,
        out: &mut W,
    ) -> Result<i32>
    where
        F: Fs + ?Sized,
        W: Write,
    {
        push_dry_run(
            DryRunPorts {
                fs,
                cwd,
                producer: &FakeProducer::ok(),
                builder: &FakeBuilder::unconsultable(),
                image_limit: lns_artifact::image::DEFAULT_IMAGE_LIMIT_BYTES,
                rebuild: false,
            },
            doc,
            reference,
            out,
        )
        .await
    }

    /// The service's half of a push, scripted: what the key answers, and whether this machine holds the image behind it.
    struct FakeBuilder {
        key: String,
        label: String,
        reused: bool,
        image: Option<lns_ipc::PushableImage>,
        failure: Option<String>,
        asked: RefCell<Vec<bool>>,
        /// What a resolution of the document answers with; `None` refuses it.
        merged: Option<String>,
        resolved: RefCell<Vec<String>>,
    }

    impl FakeBuilder {
        /// A builder no scenario may consult: a push that passes with it proves the document named no Containerfile.
        fn unconsultable() -> Self {
            Self::answering(None)
        }

        fn answering(image: Option<lns_ipc::PushableImage>) -> Self {
            Self {
                key: format!("sha256:{}", "5f".repeat(32)),
                label: "./image/Containerfile".to_string(),
                reused: false,
                image,
                failure: None,
                asked: RefCell::new(Vec::new()),
                merged: None,
                resolved: RefCell::new(Vec::new()),
            }
        }

        fn merging(mut self, definition: &str) -> Self {
            self.merged = Some(definition.to_string());
            self
        }

        fn built(layers: &[(&str, u64)]) -> Self {
            Self::answering(Some(image_of(layers, IMAGE_CONFIG)))
        }

        fn reused(mut self) -> Self {
            self.reused = true;
            self
        }

        fn failing(message: &str) -> Self {
            Self {
                failure: Some(message.to_string()),
                ..Self::answering(None)
            }
        }
    }

    const IMAGE_CONFIG: &str =
        r#"{"history":[{"created_by":"FROM alpine"},{"created_by":"RUN npm install -g agent"}]}"#;

    fn image_of(layers: &[(&str, u64)], config: &str) -> lns_ipc::PushableImage {
        lns_ipc::PushableImage {
            reference: format!("lns-build.local/built@sha256:{}", "cc".repeat(32)),
            digest: format!("sha256:{}", "cc".repeat(32)),
            manifest: "{}".into(),
            manifest_media_type: "application/vnd.oci.image.manifest.v1+json".into(),
            os: lns_artifact::image_index::OS.into(),
            architecture: "arm64".into(),
            config: config.to_string(),
            config_digest: format!("sha256:{}", "dd".repeat(32)),
            config_media_type: "application/vnd.oci.image.config.v1+json".into(),
            layers: layers
                .iter()
                .map(|(digest, size)| lns_ipc::PushableLayer {
                    digest: (*digest).to_string(),
                    media_type: "application/vnd.oci.image.layer.v1.tar+gzip".into(),
                    size: *size,
                    path: format!("/layers/{digest}"),
                })
                .collect(),
        }
    }

    impl ImageBuilder for FakeBuilder {
        fn resolve<'a>(
            &'a self,
            document: &'a [u8],
            _project_dir: &'a Path,
        ) -> LocalBoxFuture<'a, Result<ResolvedDefinition>> {
            self.resolved
                .borrow_mut()
                .push(String::from_utf8_lossy(document).into_owned());
            let merged = self.merged.clone();
            Box::pin(async move {
                let definition = merged.ok_or_else(|| {
                    anyhow::anyhow!("./project-egress.yaml: no such mixin beside the document")
                })?;
                Ok(ResolvedDefinition {
                    definition: definition.into_bytes(),
                    authored_egress: Some(r#"{"http":[]}"#.to_string()),
                    packed_filesets: Vec::new(),
                })
            })
        }

        fn build<'a>(
            &'a self,
            request: &'a ImageRequest<'a>,
        ) -> LocalBoxFuture<'a, Result<BuiltImage>> {
            self.asked.borrow_mut().push(request.plan_only);
            let failure = self.failure.clone();
            let built = BuiltImage {
                key: self.key.clone(),
                label: self.label.clone(),
                reused: self.reused,
                image: self.image.clone(),
            };
            Box::pin(async move {
                match failure {
                    Some(message) => Err(anyhow::anyhow!(message)),
                    None => Ok(built),
                }
            })
        }
    }

    /// The document a push builds an image for, and the context beside it.
    const WITH_A_CONTAINERFILE: &[u8] = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"./image"}}"#;

    fn fs_with_a_context() -> MapFs {
        MapFs::with(&[
            ("/work/image/Containerfile", "FROM alpine\nRUN true\n"),
            ("/work/image/app/main.js", "console.log(1)\n"),
        ])
    }

    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeProducer {
        failure: Option<String>,
        uploaded: RefCell<Vec<BuiltArtifact>>,
        /// Every image upload, as the tag it landed under.
        images: RefCell<Vec<(lns_ipc::PushableImage, String)>>,
        image_failure: Option<String>,
        /// What the registry holds under each per-architecture image tag, which is what a push assembles its index over.
        held: RefCell<std::collections::HashMap<String, lns_artifact::image_index::IndexEntry>>,
        indexes: RefCell<Vec<(String, Vec<u8>)>>,
        /// The index bytes each index tag names today: what a push seeds its entries from, and what decides whether a push that adds no entry still has an index to repair.
        published_indexes: RefCell<std::collections::HashMap<String, Vec<u8>>>,
        index_failure: Option<String>,
        read_failure: Option<String>,
    }

    impl FakeProducer {
        fn ok() -> Self {
            Self::default()
        }

        fn err(message: &str) -> Self {
            Self {
                failure: Some(message.to_string()),
                ..Default::default()
            }
        }

        /// A repository another architecture has already pushed to, which is what a second push adds its entry beside; its index tag names what those entries assemble to, as the push that wrote them left it.
        fn holding(self, tag: &str, architecture: &str, digest: &str) -> Self {
            self.held.borrow_mut().insert(
                format!("ghcr.io/team/hermes:{tag}"),
                lns_artifact::image_index::IndexEntry {
                    digest: digest.to_string(),
                    size: 2,
                    media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
                    os: lns_artifact::image_index::OS.to_string(),
                    architecture: architecture.to_string(),
                },
            );
            let entries = lns_artifact::image_index::in_index_order(
                self.held.borrow().values().cloned().collect(),
            );
            self.published_indexes.borrow_mut().insert(
                "ghcr.io/team/hermes:1.4.0-image".to_string(),
                lns_artifact::image_index::assemble(&entries)
                    .expect("assembling")
                    .bytes,
            );
            self
        }

        /// A repository whose index tag was left behind by a push that could not see every entry — the race §6.2 names.
        fn with_a_stale_index(self) -> Self {
            self.publishing_an_index_over(&[lns_artifact::image_index::IndexEntry {
                digest: format!("sha256:{}", "1a".repeat(32)),
                size: 2,
                media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
                os: lns_artifact::image_index::OS.to_string(),
                architecture: "amd64".to_string(),
            }])
        }

        /// A repository whose index tag names exactly these entries, whoever wrote them — which is how an entry no tag of this lns owns is found.
        fn publishing_an_index_over(
            self,
            entries: &[lns_artifact::image_index::IndexEntry],
        ) -> Self {
            self.published_indexes.borrow_mut().insert(
                "ghcr.io/team/hermes:1.4.0-image".to_string(),
                lns_artifact::image_index::assemble(entries)
                    .expect("assembling")
                    .bytes,
            );
            self
        }

        /// The index this push assembled, as the registry received it.
        fn published_index(&self) -> Vec<lns_artifact::image_index::IndexEntry> {
            let indexes = self.indexes.borrow();
            let (_, bytes) = indexes.last().expect("an index was published");
            lns_artifact::image_index::parse(bytes).expect("the index reads back")
        }

        /// The document the registry received, which is what a consumer will read.
        fn published(&self) -> serde_json::Value {
            let uploaded = self.uploaded.borrow();
            let built = uploaded.first().expect("an artifact was uploaded");
            let config = built
                .blobs
                .iter()
                .find(|blob| blob.media_type.ends_with(".config.v1+json"))
                .expect("the config blob travels with the artifact");
            serde_json::from_slice(&config.data).expect("the config blob is json")
        }

        fn packed_layers(&self) -> usize {
            self.uploaded
                .borrow()
                .first()
                .map(|built| built.fileset_layers().count())
                .unwrap_or_default()
        }
    }

    impl Producer for FakeProducer {
        fn push_built<'a>(
            &'a self,
            built: &'a BuiltArtifact,
            _reference: &'a str,
        ) -> LocalBoxFuture<'a, Result<()>> {
            self.uploaded.borrow_mut().push(built.clone());
            let failure = self.failure.clone();
            Box::pin(async move {
                match failure {
                    Some(message) => Err(anyhow::anyhow!(message)),
                    None => Ok(()),
                }
            })
        }

        fn push_image<'a>(
            &'a self,
            image: &'a lns_ipc::PushableImage,
            repository: &'a str,
            tag: &'a str,
        ) -> LocalBoxFuture<'a, Result<()>> {
            self.images
                .borrow_mut()
                .push((image.clone(), format!("{repository}:{tag}")));
            self.held
                .borrow_mut()
                .insert(format!("{repository}:{tag}"), entry_of(image));
            let failure = self.image_failure.clone();
            Box::pin(async move {
                match failure {
                    Some(message) => Err(anyhow::anyhow!(message)),
                    None => Ok(()),
                }
            })
        }

        fn image_at<'a>(
            &'a self,
            repository: &'a str,
            tag: &'a str,
        ) -> LocalBoxFuture<'a, Result<Option<lns_artifact::image_index::IndexEntry>>> {
            let held = self
                .held
                .borrow()
                .get(&format!("{repository}:{tag}"))
                .cloned();
            let failure = self.read_failure.clone();
            Box::pin(async move {
                match failure {
                    Some(message) => Err(anyhow::anyhow!(message)),
                    None => Ok(held),
                }
            })
        }

        fn index_at<'a>(
            &'a self,
            repository: &'a str,
            tag: &'a str,
        ) -> LocalBoxFuture<'a, Result<Option<lns_artifact::image_index::HeldIndex>>> {
            let bytes = self
                .published_indexes
                .borrow()
                .get(&format!("{repository}:{tag}"))
                .cloned();
            Box::pin(async move {
                match bytes {
                    Some(bytes) => lns_artifact::image_index::held(&bytes),
                    None => Ok(None),
                }
            })
        }

        fn push_index<'a>(
            &'a self,
            repository: &'a str,
            tag: &'a str,
            index: &'a [u8],
        ) -> LocalBoxFuture<'a, Result<()>> {
            if let Some(failure) = self.index_failure.clone() {
                return Box::pin(async move { Err(anyhow::anyhow!(failure)) });
            }
            self.indexes
                .borrow_mut()
                .push((format!("{repository}:{tag}"), index.to_vec()));
            self.published_indexes
                .borrow_mut()
                .insert(format!("{repository}:{tag}"), index.to_vec());
            Box::pin(async move { Ok(()) })
        }
    }

    use crate::artifact::test_support::MapFs;

    struct FakeResolver {
        versions: std::collections::HashMap<String, String>,
        verifications: std::collections::HashMap<String, IndexVerification>,
    }

    impl FakeResolver {
        fn with(entries: &[(&str, &str)]) -> Self {
            Self {
                versions: entries
                    .iter()
                    .map(|(spec, exact)| (spec.to_string(), exact.to_string()))
                    .collect(),
                verifications: std::collections::HashMap::new(),
            }
        }

        fn verifying(mut self, spec: &str, verification: IndexVerification) -> Self {
            self.verifications.insert(spec.to_string(), verification);
            self
        }
    }

    impl ToolResolver for FakeResolver {
        fn resolve<'a>(
            &'a self,
            tool: &'a lns_artifact::tools::ToolRef,
        ) -> LocalBoxFuture<'a, Result<String>> {
            let outcome = self
                .versions
                .get(&tool.to_string())
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!("tool {:?} is unknown to the version index", tool.name)
                });
            Box::pin(async move { outcome })
        }

        fn verify<'a>(
            &'a self,
            tool: &'a lns_artifact::tools::ToolRef,
        ) -> LocalBoxFuture<'a, IndexVerification> {
            let verification = self
                .verifications
                .get(&tool.to_string())
                .copied()
                .unwrap_or(IndexVerification::Unavailable);
            Box::pin(async move { verification })
        }
    }

    /// An empty index: any resolution attempt fails, so a passing push proves the resolver was never consulted.
    fn unconsultable() -> FakeResolver {
        FakeResolver::with(&[])
    }

    fn fs_with_skills() -> MapFs {
        MapFs::with(&[("/work/skills/prompts.md", "p")])
    }

    const VALID: &[u8] =
        br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1"}}"#;

    const WITH_PATH_FILESET: &[u8] = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","filesets":[{"path":"./skills","guestPath":"/root/.agent/skills"}]}}"#;

    fn cwd() -> &'static Path {
        Path::new("/work")
    }

    fn unsupported_backend_doc(with_fileset: bool) -> (String, Vec<u8>) {
        let tool = lns_artifact::tools::registry::backends()
            .find(|(_, backend)| !lns_artifact::tools::registry::is_supported_backend(backend))
            .map(|(name, _)| name.to_string())
            .expect("the snapshot carries at least one unsupported-backend entry");
        let fileset = if with_fileset {
            r#","filesets":[{"path":"./skills","guestPath":"/root/.agent/skills"}]"#
        } else {
            ""
        };
        let doc = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{{"image":"ghcr.io/team/base:1"{fileset},"tools":["{tool}@1"]}}}}"#
        )
        .into_bytes();
        (tool, doc)
    }

    /// A sandbox layering on a local mixin whose own tool no consumer could provision.
    fn fs_with_unprovisionable_mixin() -> (MapFs, Vec<u8>) {
        let tool = lns_artifact::tools::registry::backends()
            .find(|(_, backend)| !lns_artifact::tools::registry::is_supported_backend(backend))
            .map(|(name, _)| name.to_string())
            .expect("the snapshot carries at least one unsupported-backend entry");
        let fs = MapFs::with(&[(
            "/work/mixins/pg/lns.yaml",
            &format!(
                "apiVersion: lns.run/v1\nkind: mixin\nname: postgres-tools\nspec:\n  tools:\n    - {tool}@1\n"
            ),
        )]);
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","mixins":["./mixins/pg/"]}}"#.to_vec();
        (fs, doc)
    }

    #[tokio::test]
    async fn push_refuses_a_mixin_declaring_a_tool_no_consumer_could_provision() {
        let (fs, doc) = fs_with_unprovisionable_mixin();
        let producer = FakeProducer::err("must not reach the producer");
        let mut out = Vec::new();
        let err = push_no_prompt(
            &fs,
            cwd(),
            &producer,
            &unconsultable(),
            &doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("no consumer can start") && text.contains("./mixins/pg/"),
            "a mixin publishes as its own artifact, so layering on it must not smuggle past the gate the same document faces when pushed directly; got: {text}"
        );
    }

    #[tokio::test]
    async fn a_dry_run_refuses_a_mixin_declaring_a_tool_no_consumer_could_provision() {
        let (fs, doc) = fs_with_unprovisionable_mixin();
        let mut out = Vec::new();
        let err = dry_run(&fs, cwd(), &doc, "ghcr.io/team/hermes:1.4.0", &mut out)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("no consumer can start"),
            "a preview that reports a graph the real push would refuse is worse than no preview; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_builds_then_reports_the_pushed_reference() {
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        let code = push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            VALID,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("built"), "got: {text}");
        assert!(text.contains("ghcr.io/team/hermes:1.4.0"), "got: {text}");
        assert_eq!(
            producer.packed_layers(),
            0,
            "a document declaring no path fileset publishes config-only"
        );
    }

    #[tokio::test]
    async fn push_surfaces_a_producer_failure_naming_the_host() {
        let producer = FakeProducer::err("credential for ghcr.io lacks push scope");
        let mut out = Vec::new();
        let err = push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            VALID,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("push scope"), "got: {msg}");
        assert!(msg.contains("ghcr.io"), "got: {msg}");
    }

    #[tokio::test]
    async fn push_refuses_an_invalid_sandbox_before_uploading() {
        let producer = FakeProducer::err("must not reach the producer");
        let mut out = Vec::new();
        let err = push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{}}"#,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid document"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_packs_a_path_fileset_into_a_layer_of_the_same_artifact() {
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        let code = push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            WITH_PATH_FILESET,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            producer.uploaded.borrow().len(),
            1,
            "a fileset is not a separate artifact, so one push is one upload"
        );
        assert_eq!(producer.packed_layers(), 1);

        let entry = &producer.published()["spec"]["filesets"][0];
        assert_eq!(
            entry["path"], "./skills",
            "the published entry keeps its path; the content is now part of this artifact's digest (docs/sandbox-spec.md §6)"
        );
        assert_eq!(entry["guestPath"], "/root/.agent/skills");
        assert!(
            entry.get("ref").is_none(),
            "a fileset is not a separate artifact, so nothing in the published entry names one: {entry}"
        );

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("packed fileset ./skills -> sha256:"),
            "the publisher sees the digest their directory shipped under: {text}"
        );
    }

    #[test]
    fn packing_reads_each_declared_directory_in_declaration_order() {
        let fs = MapFs::with(&[
            ("/work/skills/prompts.md", "p"),
            ("/work/hooks/run.sh", "h"),
        ]);
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/opt/skills","owner":"root"},{"inline":{"a.md":"x"},"guestPath":"/notes"},{"path":"./hooks","guestPath":"/opt/hooks"}]}}"#;
        let packed = pack_path_filesets(&fs, cwd(), doc).unwrap();
        assert_eq!(
            packed
                .iter()
                .map(|dir| dir.path.as_str())
                .collect::<Vec<_>>(),
            ["./skills", "./hooks"],
            "the i-th path entry owns the i-th layer, so an inline entry in between must not consume a layer"
        );
    }

    const CONNECTOR_WITH_A_PATH_FILESET: &[u8] = br#"{"apiVersion":"lns.run/v1","kind":"connector","name":"some-provider","spec":{"serves":["api.some-provider.example"],"methods":[{"name":"token","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}],"filesets":[{"path":"./some-provider","guestPath":"~/.some-provider"}]}]}}"#;

    #[test]
    fn packing_reads_a_connectors_secret_shaped_file_so_push_can_check_its_content() {
        let fs = MapFs::with(&[(
            "/work/some-provider/credentials.json",
            r#"{"token":"some_LNSPLACEHOLDER0000000000"}"#,
        )]);
        let packed = pack_path_filesets(&fs, cwd(), CONNECTOR_WITH_A_PATH_FILESET).expect(
            "§3.2.3 exempts a connector from the refusal this name draws in every other kind",
        );
        assert_eq!(
            packed[0]
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["credentials.json"],
            "the bytes have to reach the packer, or the placeholder check push runs over them has nothing to read"
        );
    }

    #[test]
    fn packing_still_refuses_a_secret_shaped_file_in_a_sandbox_fileset() {
        let fs = MapFs::with(&[("/work/skills/credentials.json", "{}")]);
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/opt/skills"}]}}"#;
        let err = pack_path_filesets(&fs, cwd(), doc).unwrap_err();
        assert!(
            format!("{err:#}").contains("secret-shaped file"),
            "only a connector earns the §3.2.3 exception; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_refuses_a_connector_whose_packed_secret_shaped_file_declares_no_placeholder() {
        let producer = FakeProducer::ok();
        let fs = MapFs::with(&[(
            "/work/some-provider/credentials.json",
            r#"{"token":"sk-live-real"}"#,
        )]);
        let mut out = Vec::new();
        let err = push_no_prompt(
            &fs,
            cwd(),
            &producer,
            &unconsultable(),
            CONNECTOR_WITH_A_PATH_FILESET,
            "ghcr.io/team/some-provider:1.0.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("carries no placeholder"),
            "with the name refusal lifted for a connector, the placeholder rule is what keeps a real token out of the registry; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_keeps_an_explicit_root_owner_on_the_published_entry() {
        let producer = FakeProducer::ok();
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/opt/skills","owner":"root"}]}}"#;
        let mut out = Vec::new();
        push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(
            producer.published()["spec"]["filesets"][0]["owner"],
            "root",
            "owner pins inputs the workload must not touch, so publishing must not drop it"
        );
    }

    #[tokio::test]
    async fn push_publishes_a_mixin_rather_than_refusing_a_kind_it_does_not_run() {
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        let code = push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            br#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"postgres-tools","spec":{"env":{"MODE":"research"}}}"#,
            "ghcr.io/acme/postgres-tools:1.4.0",
            &mut out,
        )
        .await
        .expect("a mixin is an artifact, published like any other");
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("ghcr.io/acme/postgres-tools:1.4.0"),
            "got: {text}"
        );
    }

    const WITH_TOOLS: &[u8] = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["node@22","python@latest"]}}"#;

    #[tokio::test]
    async fn push_pins_resolved_tool_versions_into_the_published_config() {
        let producer = FakeProducer::ok();
        let resolver = FakeResolver::with(&[("node@22", "22.11.0"), ("python@latest", "3.12.6")]);
        let mut out = Vec::new();
        let code = push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &resolver,
            WITH_TOOLS,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let published = producer.published();
        assert_eq!(
            published["spec"]["tools"],
            serde_json::json!(["node@22.11.0", "python@3.12.6"])
        );
        assert_eq!(published["spec"]["image"], "ghcr.io/team/base:1");
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("pinned node@22 → node@22.11.0")
                && text.contains("pinned python@latest → python@3.12.6"),
            "the publisher sees the versions they shipped: {text}"
        );
    }

    #[tokio::test]
    async fn push_refuses_a_tool_no_consumer_could_provision() {
        let producer = FakeProducer::ok();
        let (unsupported, doc) = unsupported_backend_doc(false);
        let mut out = Vec::new();
        let err = push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            &doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("no consumer can start")
                && format!("{err:#}").contains("bring it via spec.image")
                && format!("{err:#}").contains(&unsupported),
            "got: {err:#}"
        );
        assert!(
            producer.uploaded.borrow().is_empty(),
            "nothing is uploaded and the index is never consulted"
        );
    }

    #[tokio::test]
    async fn push_dry_run_refuses_a_tool_no_consumer_could_provision() {
        let (unsupported, doc) = unsupported_backend_doc(true);
        let mut out = Vec::new();
        let err = dry_run(
            &fs_with_skills(),
            cwd(),
            &doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();

        let message = format!("{err:#}");
        assert!(
            message.contains("no consumer can start")
                && message.contains("bring it via spec.image")
                && message.contains(&unsupported),
            "got: {message}"
        );
        assert!(
            out.is_empty(),
            "a refused dry run must not print a successful push preview"
        );
    }

    #[tokio::test]
    async fn an_already_exact_pin_publishes_when_the_index_is_unavailable() {
        // Re-publishing must not fail because the index is blocked or has dropped that version.
        let producer = FakeProducer::ok();
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["node@22.11.0"]}}"#;
        let mut out = Vec::new();
        push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(
            producer.published()["spec"]["tools"],
            serde_json::json!(["node@22.11.0"])
        );
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("pinned node@22.11.0"),
            "the entry is still disclosed as published"
        );
    }

    #[tokio::test]
    async fn an_exact_pin_the_index_no_longer_lists_warns_and_still_publishes() {
        // Best-effort verification: the publisher hears about a likely typo, but the index has no veto — it also drops old valid versions.
        let producer = FakeProducer::ok();
        let resolver =
            FakeResolver::with(&[]).verifying("java@temurin-9.9.9+9", IndexVerification::Absent);
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["java@temurin-9.9.9+9"]}}"#;
        let mut out = Vec::new();
        let code = push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &resolver,
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0, "the warning never blocks the push");
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("warning") && text.contains("java@temurin-9.9.9+9"),
            "got: {text}"
        );
        assert_eq!(
            producer.published()["spec"]["tools"],
            serde_json::json!(["java@temurin-9.9.9+9"])
        );
    }

    #[tokio::test]
    async fn a_confirmed_exact_pin_publishes_silently() {
        let producer = FakeProducer::ok();
        let resolver = FakeResolver::with(&[])
            .verifying("java@temurin-21.0.5+11.0.LTS", IndexVerification::Confirmed);
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["java@temurin-21.0.5+11.0.LTS"]}}"#;
        let mut out = Vec::new();
        push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &resolver,
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("warning"), "got: {text}");
    }

    #[tokio::test]
    async fn a_vendor_exact_pin_publishes_when_the_index_is_unavailable() {
        // The resolver itself emits vendor versions, so a re-push of its own output must not need the index back.
        let producer = FakeProducer::ok();
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["java@temurin-21.0.5+11.0.LTS"]}}"#;
        let mut out = Vec::new();
        push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(
            producer.published()["spec"]["tools"],
            serde_json::json!(["java@temurin-21.0.5+11.0.LTS"])
        );
        assert!(
            !String::from_utf8_lossy(&out).contains("warning"),
            "an unanswerable index is not the publisher's problem"
        );
    }

    #[tokio::test]
    async fn push_refuses_when_the_index_lacks_the_tool() {
        let producer = FakeProducer::err("must not reach the producer");
        let resolver = FakeResolver::with(&[]);
        let mut out = Vec::new();
        let err = push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &resolver,
            WITH_TOOLS,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("resolving node@22") && msg.contains("unknown to the version index"),
            "got: {msg}"
        );
        assert!(producer.uploaded.borrow().is_empty(), "nothing must upload");
    }

    #[tokio::test]
    async fn push_without_tools_never_consults_the_resolver() {
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        push_no_prompt(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            VALID,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn push_dry_run_notes_that_tool_versions_resolve_at_push_time() {
        let mut out = Vec::new();
        dry_run(
            &fs_with_skills(),
            cwd(),
            WITH_TOOLS,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("note: 2 tool version(s) resolve at push time"),
            "got: {text}"
        );
        let mut quiet = Vec::new();
        dry_run(
            &fs_with_skills(),
            cwd(),
            VALID,
            "ghcr.io/team/hermes:1.4.0",
            &mut quiet,
        )
        .await
        .unwrap();
        let text = String::from_utf8(quiet).unwrap();
        assert!(!text.contains("note:"), "got: {text}");
    }

    #[tokio::test]
    async fn a_dry_run_of_already_pinned_tools_promises_the_digest_it_previewed() {
        // push short-circuits exact pins without consulting the index, so the real push produces the same bytes — warning otherwise defeats the point of an offline preview.
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["node@22.11.0","python@3.12.6","java@temurin-21.0.5+11.0.LTS"]}}"#;
        let mut out = Vec::new();
        dry_run(
            &fs_with_skills(),
            cwd(),
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("may differ"), "got: {text}");

        let mixed = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["node@22.11.0","jq@latest"]}}"#;
        let mut mixed_out = Vec::new();
        dry_run(
            &fs_with_skills(),
            cwd(),
            mixed,
            "ghcr.io/team/hermes:1.4.0",
            &mut mixed_out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(mixed_out).unwrap();
        assert!(
            text.contains("note: 1 tool version(s) resolve at push time"),
            "the one fuzzy entry is still called out: {text}"
        );
    }

    #[tokio::test]
    async fn pin_declared_tools_leaves_an_empty_tool_list_untouched() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1","tools":[]}}"#;
        let (pinned, reported) = pin_declared_tools(&unconsultable(), doc).await.unwrap();
        assert_eq!(pinned, doc.to_vec());
        assert!(reported.is_empty());
    }

    #[tokio::test]
    async fn pin_declared_tools_refuses_a_non_string_entry() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1","tools":[42]}}"#;
        let err = pin_declared_tools(&unconsultable(), doc).await.unwrap_err();
        assert!(format!("{err:#}").contains("not a string"), "got: {err:#}");
    }

    #[tokio::test]
    async fn a_symlinked_readme_refuses_the_push_before_anything_uploads() {
        // A README publishes automatically, so a checkout with README.md → ~/.aws/credentials must not exfiltrate the target — the same boundary fileset packing draws.
        let mut fs = fs_with_skills();
        fs.symlinks
            .insert(std::path::PathBuf::from("/work/README.md"));
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        let err = push_no_prompt(
            &fs,
            cwd(),
            &producer,
            &unconsultable(),
            VALID,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("symlink"), "got: {err:#}");
        assert!(
            producer.uploaded.borrow().is_empty(),
            "the symlink target must never reach the registry"
        );
    }

    #[tokio::test]
    async fn a_build_that_answers_with_no_image_stops_the_push_before_it_publishes_anything() {
        // The invariant of §6: a push never publishes a document whose image it does not have.
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        let err = push_with_builder(
            &fs_with_a_context(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::answering(None),
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("no image"), "got: {err:#}");
        assert!(
            producer.uploaded.borrow().is_empty(),
            "the document must not reach the registry without the image it names"
        );
    }

    #[tokio::test]
    async fn a_build_the_service_refuses_stops_the_push_with_what_it_said() {
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        let err = push_with_builder(
            &fs_with_a_context(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::failing("line 2: RUN npm install: the build guest exited 1"),
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("exited 1"), "got: {err:#}");
        assert!(producer.uploaded.borrow().is_empty());
    }

    /// The document a push builds an image for, with a mixin its build steps must be held to.
    const WITH_A_CONTAINERFILE_AND_A_MIXIN: &[u8] = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"./image","mixins":["./project-egress.yaml"]}}"#;

    fn fs_with_a_context_and_a_mixin() -> MapFs {
        MapFs::with(&[
            ("/work/image/Containerfile", "FROM alpine\nRUN true\n"),
            ("/work/image/app/main.js", "console.log(1)\n"),
            (
                "/work/project-egress.yaml",
                "apiVersion: lns.run/v1\nkind: mixin\nname: project-egress\nspec: {}\n",
            ),
        ])
    }

    #[tokio::test]
    async fn a_resolution_the_service_refuses_stops_the_push_before_it_publishes_anything() {
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        let err = push_with_builder(
            &fs_with_a_context_and_a_mixin(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::built(&[("sha256:aa", 64)]),
            WITH_A_CONTAINERFILE_AND_A_MIXIN,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("no such mixin"), "got: {err:#}");
        assert!(producer.uploaded.borrow().is_empty());
        assert!(producer.images.borrow().is_empty());
    }

    #[tokio::test]
    async fn a_dry_run_of_a_document_with_a_mixin_plans_the_merged_document() {
        let builder = FakeBuilder::built(&[("sha256:aa", 64)])
            .reused()
            .merging(r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"./image"}}"#);
        let mut out = Vec::new();
        let code = push_dry_run(
            DryRunPorts {
                fs: &fs_with_a_context_and_a_mixin(),
                cwd: cwd(),
                producer: &FakeProducer::ok(),
                builder: &builder,
                image_limit: lns_artifact::image::DEFAULT_IMAGE_LIMIT_BYTES,
                rebuild: false,
            },
            WITH_A_CONTAINERFILE_AND_A_MIXIN,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            builder.resolved.borrow().len(),
            1,
            "a preview plans the document a build would run, not the one on disk"
        );
    }

    /// The digest another architecture's push left behind, which this one must not replace.
    const AMD64_DIGEST: &str = "sha256:aaaa";

    #[tokio::test]
    async fn a_push_from_a_second_architecture_adds_its_entry_to_the_index_the_first_published() {
        let producer = FakeProducer::ok().holding("1.4.0-image-linux-amd64", "amd64", AMD64_DIGEST);
        let mut out = Vec::new();
        let code = push_with_builder(
            &fs_with_a_context(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::built(&[("sha256:base", 10)]),
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let published = producer.published_index();
        assert_eq!(
            published
                .iter()
                .map(|entry| (entry.platform(), entry.digest.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("linux/amd64".to_string(), AMD64_DIGEST.to_string()),
                (
                    "linux/arm64".to_string(),
                    format!("sha256:{}", "cc".repeat(32))
                ),
            ],
            "§6: a second architecture adds an entry rather than replacing the image"
        );
        let uploaded: Vec<String> = producer
            .images
            .borrow()
            .iter()
            .map(|(_, tag)| tag.clone())
            .collect();
        assert_eq!(
            uploaded,
            vec!["ghcr.io/team/hermes:1.4.0-image-linux-arm64"],
            "each architecture writes only the tag it owns"
        );
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("index ghcr.io/team/hermes@sha256:")
                && text.contains("linux/amd64 sha256:aaaa"),
            "the publisher sees what the index now holds: {text}"
        );
    }

    #[tokio::test]
    async fn the_published_document_names_the_index_rather_than_this_architecture_s_manifest() {
        let producer = FakeProducer::ok().holding("1.4.0-image-linux-amd64", "amd64", AMD64_DIGEST);
        let mut out = Vec::new();
        push_with_builder(
            &fs_with_a_context(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::built(&[("sha256:base", 10)]),
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let index = lns_artifact::image_index::assemble(&producer.published_index()).unwrap();
        assert_eq!(
            producer.published()["spec"]["image"],
            format!("ghcr.io/team/hermes@{}", index.digest),
            "a consumer's pull selects its own architecture, which only an index lets it do"
        );
    }

    #[tokio::test]
    async fn a_push_whose_architecture_the_index_already_holds_publishes_nothing_and_says_so() {
        let producer = FakeProducer::ok().holding(
            "1.4.0-image-linux-arm64",
            "arm64",
            &format!("sha256:{}", "cc".repeat(32)),
        );
        let mut out = Vec::new();
        push_with_builder(
            &fs_with_a_context(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::built(&[("sha256:base", 10)]).reused(),
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert!(
            producer.images.borrow().is_empty() && producer.indexes.borrow().is_empty(),
            "an unchanged key whose entry is already indexed uploads no image and no index"
        );
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("nothing to publish: the index already holds linux/arm64"),
            "got: {text}"
        );
    }

    #[tokio::test]
    async fn a_push_that_adds_no_entry_still_repairs_an_index_a_race_left_behind() {
        let producer = FakeProducer::ok()
            .holding(
                "1.4.0-image-linux-arm64",
                "arm64",
                &format!("sha256:{}", "cc".repeat(32)),
            )
            .with_a_stale_index();
        let mut out = Vec::new();
        push_with_builder(
            &fs_with_a_context(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::built(&[("sha256:base", 10)]).reused(),
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert!(
            producer.images.borrow().is_empty(),
            "the entry is already published, so no image is uploaded again"
        );
        let index = lns_artifact::image_index::assemble(&producer.published_index()).unwrap();
        assert_eq!(
            producer.published()["spec"]["image"],
            format!("ghcr.io/team/hermes@{}", index.digest),
            "a document must never name an index the registry does not hold"
        );
    }

    /// §6.2: a newer lns can publish an architecture this one does not build, and the tags this one reads say nothing about it, so the index it holds is where the entry survives.
    #[tokio::test]
    async fn an_entry_for_a_platform_this_lns_does_not_build_survives_the_next_push() {
        let foreign = lns_artifact::image_index::IndexEntry {
            digest: format!("sha256:{}", "1f".repeat(32)),
            size: 2,
            media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
            os: lns_artifact::image_index::OS.to_string(),
            architecture: "riscv64".to_string(),
        };
        let producer = FakeProducer::ok().publishing_an_index_over(std::slice::from_ref(&foreign));
        let mut out = Vec::new();
        push_with_builder(
            &fs_with_a_context(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::built(&[("sha256:base", 10)]),
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let published = producer.published_index();
        assert!(
            published.contains(&foreign),
            "an older lns must not drop what a newer one published: {published:?}"
        );
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("linux/riscv64") && text.contains("linux/arm64"),
            "the publisher sees every architecture the index now holds: {text}"
        );
    }

    #[tokio::test]
    async fn a_dry_run_says_what_the_index_holds_and_what_this_push_would_add() {
        let producer = FakeProducer::ok().holding("1.4.0-image-linux-amd64", "amd64", AMD64_DIGEST);
        let mut out = Vec::new();
        push_dry_run(
            DryRunPorts {
                fs: &fs_with_a_context(),
                cwd: cwd(),
                producer: &producer,
                builder: &FakeBuilder::built(&[("sha256:base", 10)]).reused(),
                image_limit: lns_artifact::image::DEFAULT_IMAGE_LIMIT_BYTES,
                rebuild: false,
            },
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("the index holds linux/amd64 sha256:aaaa"),
            "got: {text}"
        );
        assert!(
            text.contains("this push would add linux/arm64"),
            "got: {text}"
        );
        assert!(
            producer.images.borrow().is_empty() && producer.indexes.borrow().is_empty(),
            "a dry run uploads nothing"
        );
    }

    #[tokio::test]
    async fn a_dry_run_of_an_architecture_the_index_already_holds_says_it_would_add_nothing() {
        let producer = FakeProducer::ok().holding(
            "1.4.0-image-linux-arm64",
            "arm64",
            &format!("sha256:{}", "cc".repeat(32)),
        );
        let mut out = Vec::new();
        push_dry_run(
            DryRunPorts {
                fs: &fs_with_a_context(),
                cwd: cwd(),
                producer: &producer,
                builder: &FakeBuilder::built(&[("sha256:base", 10)]).reused(),
                image_limit: lns_artifact::image::DEFAULT_IMAGE_LIMIT_BYTES,
                rebuild: false,
            },
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("this push would add nothing: the index already holds linux/arm64"),
            "got: {text}"
        );
    }

    #[tokio::test]
    async fn a_dry_run_of_a_repository_nobody_has_pushed_to_says_the_index_holds_nothing_yet() {
        let mut out = Vec::new();
        push_dry_run(
            DryRunPorts {
                fs: &fs_with_a_context(),
                cwd: cwd(),
                producer: &FakeProducer::ok(),
                builder: &FakeBuilder::built(&[("sha256:base", 10)]),
                image_limit: lns_artifact::image::DEFAULT_IMAGE_LIMIT_BYTES,
                rebuild: false,
            },
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("the index holds nothing yet"), "got: {text}");
    }

    #[tokio::test]
    async fn an_index_the_registry_refuses_stops_the_push_naming_the_repository() {
        let producer = FakeProducer {
            index_failure: Some("credential for ghcr.io lacks push scope".into()),
            ..FakeProducer::ok()
        };
        let mut out = Vec::new();
        let err = push_with_builder(
            &fs_with_a_context(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::built(&[("sha256:base", 10)]),
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("image index into ghcr.io/team/hermes"),
            "a document may not name an index the registry never took: {message}"
        );
        assert!(producer.uploaded.borrow().is_empty());
    }

    #[tokio::test]
    async fn a_repository_this_push_cannot_read_stops_it_before_it_publishes_anything() {
        let producer = FakeProducer {
            read_failure: Some("registry timeout".into()),
            ..FakeProducer::ok()
        };
        let mut out = Vec::new();
        let err = push_with_builder(
            &fs_with_a_context(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::built(&[("sha256:base", 10)]),
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("ghcr.io/team/hermes:1.4.0-image-linux"),
            "an index assembled over a repository nobody could read would drop an entry: {message}"
        );
        assert!(producer.uploaded.borrow().is_empty());
    }

    #[tokio::test]
    async fn a_registry_that_refuses_the_image_names_the_repository_it_was_publishing_into() {
        let producer = FakeProducer {
            image_failure: Some("credential for ghcr.io lacks push scope".into()),
            ..FakeProducer::ok()
        };
        let mut out = Vec::new();
        let err = push_with_builder(
            &fs_with_a_context(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::built(&[("sha256:base", 10)]),
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("ghcr.io/team/hermes"), "{message}");
        assert!(message.contains("push scope"), "{message}");
        assert!(producer.uploaded.borrow().is_empty());
    }

    #[tokio::test]
    async fn an_image_of_one_layer_is_reported_in_the_singular() {
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        push_with_builder(
            &fs_with_a_context(),
            cwd(),
            &producer,
            &unconsultable(),
            &FakeBuilder::built(&[("sha256:only", 10)]).reused(),
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("(1 layer)"), "got: {text}");
        assert!(text.contains("reused ./image/Containerfile"), "got: {text}");
    }

    #[tokio::test]
    async fn a_dry_run_that_knows_the_digest_previews_the_document_a_push_would_publish() {
        let mut out = Vec::new();
        push_dry_run(
            DryRunPorts {
                fs: &fs_with_a_context(),
                cwd: cwd(),
                producer: &FakeProducer::ok(),
                builder: &FakeBuilder::built(&[("sha256:base", 10)]),
                image_limit: lns_artifact::image::DEFAULT_IMAGE_LIMIT_BYTES,
                rebuild: false,
            },
            WITH_A_CONTAINERFILE,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("would pack ./image/Containerfile -> sha256:"),
            "the Containerfile is a layer of the artifact, so a preview names its digest too: {text}"
        );
    }

    #[tokio::test]
    async fn a_dry_run_previews_the_layer_digest_each_directory_would_publish_under() {
        let mut out = Vec::new();
        dry_run(
            &fs_with_skills(),
            cwd(),
            WITH_PATH_FILESET,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("would pack fileset ./skills -> sha256:"),
            "a dry run says what would publish, and the layer digest is part of what the manifest would carry: {text}"
        );
        assert!(text.contains("nothing uploaded"), "got: {text}");
    }
}
