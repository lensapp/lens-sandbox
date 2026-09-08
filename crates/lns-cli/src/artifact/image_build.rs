use std::path::Path;

use anyhow::{Result, bail};

use lns_artifact::image::{CONTAINERFILE_NAMES, ImageSource, source};

use super::author::Fs;

/// The Containerfile a path-form `spec.image` names, spelled as the author wrote it: a directory holds one under either name — `Containerfile` first, as Podman does — and a path naming a file is the Containerfile whatever it is called (§3.1.1).
pub fn containerfile<F: Fs + ?Sized>(fs: &F, project_dir: &Path, path: &str) -> Result<String> {
    let named = lns_artifact::sandbox::fold_path(&project_dir.join(path));
    let stem = path.trim_end_matches('/');
    if fs.is_dir(&named) {
        return CONTAINERFILE_NAMES
            .iter()
            .find(|name| fs.exists(&named.join(name)))
            .map(|name| format!("{stem}/{name}"))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "spec.image {path:?} names a directory holding no Containerfile or Dockerfile"
                )
            });
    }
    if fs.exists(&named) {
        return Ok(stem.to_string());
    }
    bail!("spec.image {path:?} names no file or directory beside the document")
}

/// The offline validate/inspect guard: a path-form image must name a Containerfile this machine can read, so a typo is found where a fileset's is.
pub fn image_problems<F: Fs + ?Sized>(fs: &F, project_dir: &Path, image: &str) -> Vec<String> {
    match built_from(fs, project_dir, image) {
        Ok(_) => Vec::new(),
        Err(e) => vec![format!("{e:#}")],
    }
}

/// The Containerfile line a render discloses, and `None` for an image that is pulled rather than built.
pub fn built_from<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    image: &str,
) -> Result<Option<String>> {
    match source(image) {
        ImageSource::Reference(_) => Ok(None),
        ImageSource::Containerfile(path) => containerfile(fs, project_dir, path).map(Some),
    }
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
