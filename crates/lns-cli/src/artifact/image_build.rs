use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use lns_artifact::image::{CONTAINERFILE_NAMES, ImageSource, source};

use super::author::Fs;

/// What a path-form `spec.image` discloses before anything is built: the Containerfile lns would build, its text, and the files the context would carry (§3.1.1).
pub struct BuiltImage {
    pub containerfile: String,
    pub text: String,
    pub context: Vec<ContextFile>,
}

/// One file of the build context, as an approver reads it: the path inside the context and its size.
pub struct ContextFile {
    pub path: String,
    pub bytes: u64,
}

impl BuiltImage {
    /// The image line of a summary: which file builds this sandbox, and how much there is to read.
    pub fn summary(&self) -> String {
        format!(
            "built from {} ({} lines, context {} files)",
            self.containerfile,
            self.text.lines().count(),
            self.context.len()
        )
    }
}

/// The Containerfile a path-form `spec.image` names, spelled as the author wrote it and paired with the absolute path this machine reads it at: a directory holds one under either name — `Containerfile` first, as Podman does — and a path naming a file is the Containerfile whatever it is called (§3.1.1).
fn resolve<F: Fs + ?Sized>(fs: &F, project_dir: &Path, path: &str) -> Result<(String, PathBuf)> {
    let named = lns_artifact::sandbox::fold_path(&project_dir.join(path));
    let stem = path.trim_end_matches('/');
    if fs.is_dir(&named) {
        return CONTAINERFILE_NAMES
            .iter()
            .find(|name| fs.exists(&named.join(name)))
            .map(|name| (format!("{stem}/{name}"), named.join(name)))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "spec.image {path:?} names a directory holding no Containerfile or Dockerfile"
                )
            });
    }
    if fs.exists(&named) {
        return Ok((stem.to_string(), named));
    }
    bail!("spec.image {path:?} names no file or directory beside the document")
}

/// The offline validate/inspect guard: a path-form image must name a Containerfile this machine can read and lns can build, so a typo and an unbuildable instruction are both found before a push.
pub fn image_problems<F: Fs + ?Sized>(fs: &F, project_dir: &Path, image: &str) -> Vec<String> {
    let built = match built_from(fs, project_dir, image) {
        Ok(None) => return Vec::new(),
        Ok(Some(built)) => built,
        Err(e) => return vec![format!("{e:#}")],
    };
    match lns_artifact::containerfile::parse(&built.text) {
        Ok(_) => Vec::new(),
        Err(refusals) => refusals
            .iter()
            .map(|refusal| format!("{} {refusal}", built.containerfile))
            .collect(),
    }
}

/// What a render discloses about the image, and `None` for one that is pulled rather than built.
pub fn built_from<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    image: &str,
) -> Result<Option<BuiltImage>> {
    let ImageSource::Containerfile(path) = source(image) else {
        return Ok(None);
    };
    let (containerfile, at) = resolve(fs, project_dir, path)?;
    let text = fs
        .read_to_string(&at)
        .with_context(|| format!("reading {containerfile}"))?;
    let context = context_files(fs, at.parent().unwrap_or(project_dir))?;
    Ok(Some(BuiltImage {
        containerfile,
        text,
        context,
    }))
}

/// Every file a build would send, because the context is what the artifact ships and an approver reads it before it is packed.
fn context_files<F: Fs + ?Sized>(fs: &F, root: &Path) -> Result<Vec<ContextFile>> {
    let mut files = Vec::new();
    collect_context(fs, root, Path::new(""), &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn collect_context<F: Fs + ?Sized>(
    fs: &F,
    dir: &Path,
    rel: &Path,
    out: &mut Vec<ContextFile>,
) -> Result<()> {
    let listed = fs
        .dir_entries(dir)
        .with_context(|| format!("reading the build context {}", dir.display()))?;
    for entry in listed {
        let entry_rel = rel.join(&entry.name);
        if entry.dir {
            collect_context(fs, &dir.join(&entry.name), &entry_rel, out)?;
            continue;
        }
        let bytes = fs
            .read_limited(
                &dir.join(&entry.name),
                lns_artifact::build::MAX_FILESET_BYTES,
            )
            .with_context(|| format!("reading the build context file {}", entry_rel.display()))?
            .len() as u64;
        out.push(ContextFile {
            path: entry_rel.display().to_string(),
            bytes,
        });
    }
    Ok(())
}

/// A path-form `spec.image` publishes as the digest of an image lns built ([§6](docs/sandbox-spec.md)), and lns has no builder yet, so the verbs that would need one refuse rather than hand a path to a registry.
pub fn refuse_an_unbuilt_image(image: &str) -> Result<()> {
    if let ImageSource::Containerfile(path) = source(image) {
        bail!(
            "spec.image {path:?} names a Containerfile, and building one is not yet supported; publish the image yourself and name it by reference"
        );
    }
    Ok(())
}
