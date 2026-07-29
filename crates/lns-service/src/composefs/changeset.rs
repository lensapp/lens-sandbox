use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

pub(crate) const MAX_NAME_BYTES: usize = 255;

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PathChange {
    Entry(PathBuf),
    Remove(PathBuf),
    ClearDirectory(PathBuf),
}

pub(crate) fn classify_path(path: &Path) -> Result<Option<PathChange>> {
    let Some(normalized) = normalize_path(path)? else {
        return Ok(None);
    };
    let name = normalized
        .file_name()
        .expect("a normalized non-empty path has a final component");
    let parent = normalized.parent().unwrap_or_else(|| Path::new(""));
    let name_bytes = name.as_bytes();
    if name_bytes == b".wh..wh..opq" {
        return Ok(Some(PathChange::ClearDirectory(parent.to_path_buf())));
    }
    if let Some(target) = strip_wh_prefix(name_bytes) {
        if target.is_empty() {
            bail!("whiteout path has no target: {}", path.display());
        }
        return Ok(Some(PathChange::Remove(parent.join(target))));
    }
    Ok(Some(PathChange::Entry(normalized)))
}

pub(crate) fn strip_wh_prefix(name: &[u8]) -> Option<&OsStr> {
    let stripped = name.strip_prefix(b".wh.")?;
    if stripped.starts_with(b".wh.") {
        return None;
    }
    Some(OsStr::from_bytes(stripped))
}

pub(crate) fn normalize_path(path: &Path) -> Result<Option<PathBuf>> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => {
                if segment.as_bytes().len() > MAX_NAME_BYTES {
                    bail!(
                        "tar entry path component exceeds the {MAX_NAME_BYTES}-byte name limit: {}",
                        path.display()
                    );
                }
                normalized.push(segment);
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                bail!("tar entry path contains `..`: {}", path.display())
            }
        }
    }
    Ok((!normalized.as_os_str().is_empty()).then_some(normalized))
}
