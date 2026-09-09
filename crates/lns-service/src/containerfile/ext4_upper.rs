use std::path::Path;

use anyhow::{Context, Result, anyhow};
use ext4_view::{Ext4, FileType};

use super::upper::{UpperEntry, UpperKind, UpperTree};

/// The overlay's upper directory inside the run's ext4 volume, as `lns-init` lays it out.
const UPPER_DIR: &str = "/upper";

/// A stopped run's `upper.img`, read where the guest left it: the kernel clears no journal on
/// `reboot(RB_POWER_OFF)`, so every read goes through `ext4-view`'s journal mapping.
pub(crate) struct Ext4Upper {
    fs: Ext4,
}

impl Ext4Upper {
    pub(crate) fn open(image: &Path) -> Result<Self> {
        let fs = Ext4::load_from_path(image)
            .map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("reading the run's upper volume {}", image.display()))?;
        Ok(Self { fs })
    }

    fn absolute(&self, path: &str) -> String {
        if path.is_empty() {
            UPPER_DIR.to_string()
        } else {
            format!("{UPPER_DIR}/{path}")
        }
    }
}

impl UpperTree for Ext4Upper {
    fn read_dir(&self, path: &str) -> Result<Vec<UpperEntry>> {
        let absolute = self.absolute(path);
        let mut entries = Vec::new();
        for entry in self
            .fs
            .read_dir(absolute.as_str())
            .map_err(|e| anyhow!("{e}"))?
        {
            let entry = entry.map_err(|e| anyhow!("{e}"))?;
            let name = entry
                .file_name()
                .as_str()
                .map_err(|e| anyhow!("{e}"))?
                .to_string();
            if name == "." || name == ".." {
                continue;
            }
            let metadata = entry.metadata().map_err(|e| anyhow!("{e}"))?;
            entries.push(UpperEntry {
                name,
                kind: kind_of(metadata.file_type()),
                mode: u32::from(metadata.mode()),
                uid: metadata.uid(),
                gid: metadata.gid(),
            });
        }
        Ok(entries)
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        self.fs
            .read(self.absolute(path).as_str())
            .map_err(|e| anyhow!("{e}"))
    }

    fn read_link(&self, path: &str) -> Result<String> {
        let target = self
            .fs
            .read_link(self.absolute(path).as_str())
            .map_err(|e| anyhow!("{e}"))?;
        Ok(target.to_str().map_err(|e| anyhow!("{e}"))?.to_string())
    }
}

/// Overlayfs writes a deletion as a 0:0 character device, so a character device here is the
/// deletion a `RUN` made; `ext4-view` reports no rdev, so no other character device can be told
/// apart from one.
fn kind_of(file_type: FileType) -> UpperKind {
    match file_type {
        FileType::Directory => UpperKind::Directory,
        FileType::Regular => UpperKind::Regular,
        FileType::Symlink => UpperKind::Symlink,
        FileType::CharacterDevice => UpperKind::Whiteout,
        _ => UpperKind::Unsupported,
    }
}
