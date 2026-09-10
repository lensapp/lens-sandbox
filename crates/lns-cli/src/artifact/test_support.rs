use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use crate::artifact::author::Fs;
use lns_artifact::walk::{DirEntry, map_dir_entries};

/// The one in-memory Fs fake shared by every sandbox-side unit suite: a flat path→contents map with an optional write failure.
#[derive(Default)]
pub(crate) struct MapFs {
    pub files: RefCell<HashMap<PathBuf, String>>,
    pub fail_write: bool,
    pub symlinks: HashSet<PathBuf>,
    /// Sizes reported instead of the held content's length, so a big file needs no bytes.
    pub sizes: HashMap<PathBuf, u64>,
    /// Every listing, sizing and read the code under test asked for, in order, so a test can pin what a check did not touch.
    pub touched: RefCell<Vec<String>>,
}

impl MapFs {
    pub fn with(entries: &[(&str, &str)]) -> Self {
        Self {
            files: RefCell::new(
                entries
                    .iter()
                    .map(|(path, contents)| (PathBuf::from(path), contents.to_string()))
                    .collect(),
            ),
            ..Self::default()
        }
    }
}

impl Fs for MapFs {
    fn is_dir(&self, path: &Path) -> bool {
        self.files
            .borrow()
            .keys()
            .chain(self.symlinks.iter())
            .any(|held| held.ancestors().skip(1).any(|dir| dir == path))
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such file"))
    }
    fn write(&self, path: &Path, contents: &str) -> io::Result<()> {
        if self.fail_write {
            return Err(io::Error::other("disk full"));
        }
        self.files
            .borrow_mut()
            .insert(path.to_path_buf(), contents.to_string());
        Ok(())
    }
    fn exists(&self, path: &Path) -> bool {
        self.files.borrow().contains_key(path) || self.symlinks.contains(path)
    }
    fn is_symlink(&self, path: &Path) -> bool {
        self.symlinks.contains(path)
    }
    fn size(&self, path: &Path) -> io::Result<u64> {
        self.touched
            .borrow_mut()
            .push(format!("size {}", path.display()));
        match self.sizes.get(path) {
            Some(size) => Ok(*size),
            None => self.read_to_string(path).map(|held| held.len() as u64),
        }
    }
}

impl lns_artifact::walk::SnapshotFs for MapFs {
    fn read_limited(&self, path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
        self.touched
            .borrow_mut()
            .push(format!("read {}", path.display()));
        let mut bytes = self.read_to_string(path)?.into_bytes();
        bytes.truncate(max_bytes.saturating_add(1) as usize);
        Ok(bytes)
    }
    fn dir_entries(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        self.touched
            .borrow_mut()
            .push(format!("list {}", dir.display()));
        let held: Vec<PathBuf> = self
            .files
            .borrow()
            .keys()
            .chain(self.symlinks.iter())
            .cloned()
            .collect();
        let mut listed = map_dir_entries(held.iter(), dir)?;
        for entry in &mut listed {
            entry.symlink = self.symlinks.contains(&dir.join(&entry.name));
        }
        Ok(listed)
    }
}
