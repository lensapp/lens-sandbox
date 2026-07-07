use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_artifact::build::FileEntry;

use super::report;

/// One entry a directory tree yields: a subdirectory to descend, a regular file to pack, or anything else (symlink, socket) to skip.
pub(super) enum Entry {
    Dir(PathBuf),
    File(PathBuf),
    Other,
}

/// A directory tree the FileSet packer walks; the real implementation is `std::fs`, and tests inject a fake to exercise every branch (nested dirs, skipped non-files, and read failures) without touching disk.
pub(super) trait DirTree {
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<Entry>>;
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>>;
}

/// Turn a directory name into a FileSet `metadata.name`.
pub(super) fn fileset_name(path: &Path) -> String {
    let raw = path.file_name().map(|n| n.to_string_lossy());
    report::sanitize_name(raw.as_deref().unwrap_or("fileset"))
}

/// Recursively collect every regular file under `root` into mount-relative `FileEntry`s, skipping symlinks and other non-regular entries.
pub(super) fn collect_dir<T: DirTree>(tree: &T, root: &Path) -> Result<Vec<FileEntry>> {
    let mut out = Vec::new();
    walk(tree, root, root, &mut out)?;
    Ok(out)
}

fn walk<T: DirTree>(tree: &T, root: &Path, dir: &Path, out: &mut Vec<FileEntry>) -> Result<()> {
    let entries = tree
        .read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        match entry {
            Entry::Dir(path) => walk(tree, root, &path, out)?,
            Entry::File(path) => {
                let rel = path
                    .strip_prefix(root)
                    .with_context(|| format!("{} escapes {}", path.display(), root.display()))?;
                let data = tree
                    .read_file(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                out.push(FileEntry {
                    path: rel.to_string_lossy().into_owned(),
                    data,
                });
            }
            Entry::Other => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct FakeTree {
        dirs: BTreeMap<PathBuf, Vec<Entry>>,
        files: BTreeMap<PathBuf, Vec<u8>>,
        unreadable: Option<PathBuf>,
    }

    impl DirTree for FakeTree {
        fn read_dir(&self, dir: &Path) -> io::Result<Vec<Entry>> {
            match self.dirs.get(dir) {
                Some(entries) => Ok(entries.iter().map(clone_entry).collect()),
                None => Err(io::Error::from(io::ErrorKind::NotFound)),
            }
        }
        fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
            if self.unreadable.as_deref() == Some(path) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    fn clone_entry(e: &Entry) -> Entry {
        match e {
            Entry::Dir(p) => Entry::Dir(p.clone()),
            Entry::File(p) => Entry::File(p.clone()),
            Entry::Other => Entry::Other,
        }
    }

    #[test]
    fn fileset_name_sanitizes_the_directory_basename() {
        assert_eq!(
            fileset_name(Path::new("/tmp/Deep.Research")),
            "deep-research"
        );
        assert_eq!(fileset_name(Path::new("/")), "fileset");
    }

    #[test]
    fn collect_dir_flattens_nested_files_relative_to_root_and_skips_non_files() {
        let mut tree = FakeTree::default();
        tree.dirs.insert(
            PathBuf::from("/root"),
            vec![
                Entry::File(PathBuf::from("/root/a.txt")),
                Entry::Dir(PathBuf::from("/root/sub")),
                Entry::Other,
            ],
        );
        tree.dirs.insert(
            PathBuf::from("/root/sub"),
            vec![Entry::File(PathBuf::from("/root/sub/b.txt"))],
        );
        tree.files
            .insert(PathBuf::from("/root/a.txt"), b"aaa".to_vec());
        tree.files
            .insert(PathBuf::from("/root/sub/b.txt"), b"bbb".to_vec());

        let mut entries = collect_dir(&tree, Path::new("/root")).unwrap();
        entries.sort_by(|x, y| x.path.cmp(&y.path));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "a.txt");
        assert_eq!(entries[1].path, "sub/b.txt");
    }

    #[test]
    fn collect_dir_surfaces_a_read_dir_failure() {
        let err = collect_dir(&FakeTree::default(), Path::new("/missing")).unwrap_err();
        assert!(format!("{err:#}").contains("reading /missing"), "{err:#}");
    }

    #[test]
    fn collect_dir_surfaces_a_file_read_failure() {
        let mut tree = FakeTree::default();
        tree.dirs.insert(
            PathBuf::from("/root"),
            vec![Entry::File(PathBuf::from("/root/x"))],
        );
        tree.unreadable = Some(PathBuf::from("/root/x"));
        let err = collect_dir(&tree, Path::new("/root")).unwrap_err();
        assert!(format!("{err:#}").contains("reading /root/x"), "{err:#}");
    }

    #[test]
    fn collect_dir_refuses_a_walked_path_that_escapes_the_root() {
        let mut tree = FakeTree::default();
        tree.dirs.insert(
            PathBuf::from("/root"),
            vec![Entry::File(PathBuf::from("/elsewhere/x"))],
        );
        let err = collect_dir(&tree, Path::new("/root")).unwrap_err();
        assert!(format!("{err:#}").contains("escapes"), "{err:#}");
    }
}
