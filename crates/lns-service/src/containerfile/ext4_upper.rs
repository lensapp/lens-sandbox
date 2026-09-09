use std::path::Path;

use anyhow::{Context, Result, anyhow};
use ext4_view::{Ext4, Ext4Read, FileType};

use super::upper::{UpperEntry, UpperKind, UpperTree};

/// The overlay's upper directory inside the run's ext4 volume, as `lns-init` lays it out.
const UPPER_DIR: &str = "/upper";

/// Where the ext4 superblock starts, and where its incompatible-feature word sits inside it.
const SUPERBLOCK_START: u64 = 1024;
const FEATURE_INCOMPAT_OFFSET: u64 = 0x60;

/// A stopped run's `upper.img`, read where the guest left it.
pub(crate) struct Ext4Upper {
    fs: Ext4,
    root: String,
}

impl Ext4Upper {
    pub(crate) fn open_run_upper(image: &Path) -> Result<Self> {
        Self::open(image, UPPER_DIR)
    }

    pub(crate) fn open(image: &Path, root: &str) -> Result<Self> {
        let file = std::fs::File::open(image)
            .with_context(|| format!("opening the run's upper volume {}", image.display()))?;
        let fs = Ext4::load(Box::new(SyncedImage { file }))
            .map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("reading the run's upper volume {}", image.display()))?;
        Ok(Self {
            fs,
            root: root.to_string(),
        })
    }

    fn absolute(&self, path: &str) -> String {
        match (self.root.as_str(), path) {
            (root, "") => root.to_string(),
            ("/", path) => format!("/{path}"),
            (root, path) => format!("{root}/{path}"),
        }
    }
}

/// The guest syncs and powers off without unmounting, so the volume still claims it needs
/// recovery through a journal `ext4-view` cannot read — but `sync(2)` has already written the home
/// blocks, so the flag comes off on the way in and the reader sees what the guest synced.
struct SyncedImage {
    file: std::fs::File,
}

impl Ext4Read for SyncedImage {
    fn read(
        &mut self,
        start_byte: u64,
        dst: &mut [u8],
    ) -> Result<(), Box<dyn core::error::Error + Send + Sync + 'static>> {
        <std::fs::File as Ext4Read>::read(&mut self.file, start_byte, dst)?;
        clear_recovery_flag(start_byte, dst);
        Ok(())
    }
}

/// The `INCOMPAT_RECOVER` bit, cleared wherever a read overlaps the superblock's feature word.
fn clear_recovery_flag(start_byte: u64, dst: &mut [u8]) {
    let word = SUPERBLOCK_START + FEATURE_INCOMPAT_OFFSET;
    let Some(offset) = word.checked_sub(start_byte) else {
        return;
    };
    let Ok(offset) = usize::try_from(offset) else {
        return;
    };
    let Some(bytes) = dst.get_mut(offset..offset + 4) else {
        return;
    };
    let cleared = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        & !crate::upperfs::FEATURE_INCOMPAT_RECOVER;
    bytes.copy_from_slice(&cleared.to_le_bytes());
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

/// Overlayfs writes a deletion as a 0:0 character device and `ext4-view` reports no rdev, so every
/// character device in the upper reads as the deletion a `RUN` made.
fn kind_of(file_type: FileType) -> UpperKind {
    match file_type {
        FileType::Directory => UpperKind::Directory,
        FileType::Regular => UpperKind::Regular,
        FileType::Symlink => UpperKind::Symlink,
        FileType::CharacterDevice => UpperKind::Whiteout,
        _ => UpperKind::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upperfs::{Plan, write_ext4};

    /// An ext4 volume shaped like a run's: this repo's writer formats it, then it is marked as
    /// mounted-and-never-unmounted, which is how every guest leaves one.
    fn unclean_image() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(32 * 1024 * 1024, [0x5A; 16], "lns-upper", 0).expect("plan");
        write_ext4(&plan, &path).expect("write_ext4");

        let mut bytes = std::fs::read(&path).expect("read image");
        let word = (SUPERBLOCK_START + FEATURE_INCOMPAT_OFFSET) as usize;
        let mut features = u32::from_le_bytes(bytes[word..word + 4].try_into().unwrap());
        features |= crate::upperfs::FEATURE_INCOMPAT_RECOVER;
        bytes[word..word + 4].copy_from_slice(&features.to_le_bytes());
        std::fs::write(&path, &bytes).expect("write image");

        (dir, path)
    }

    #[test]
    fn a_volume_the_guest_never_unmounted_still_opens() {
        let (_dir, path) = unclean_image();
        let upper = Ext4Upper::open(&path, "/").expect("the unclean volume must open");
        let names: Vec<String> = upper
            .read_dir("")
            .expect("the volume's root must read")
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert!(
            names.contains(&"lost+found".to_string()),
            "the writer's own root directory must read back, got {names:?}",
        );
    }

    #[test]
    fn the_run_upper_reads_from_the_directory_lns_init_makes_the_overlay_upper() {
        let (_dir, path) = unclean_image();
        let upper = match Ext4Upper::open_run_upper(&path) {
            Ok(upper) => upper,
            Err(e) => panic!("open: {e:#}"),
        };
        assert_eq!(upper.absolute(""), "/upper");
        assert_eq!(upper.absolute("etc/hostname"), "/upper/etc/hostname");
        let err = upper
            .read_dir("")
            .expect_err("a fresh volume has no upper dir yet");
        assert!(format!("{err:#}").contains("file not found"), "{err:#}");
    }

    #[test]
    fn a_root_of_slash_does_not_double_the_separator() {
        let (_dir, path) = unclean_image();
        let upper = Ext4Upper::open(&path, "/").expect("open");
        assert_eq!(upper.absolute("lost+found"), "/lost+found");
    }

    #[test]
    fn a_missing_file_and_a_missing_symlink_are_reported_not_panicked() {
        let (_dir, path) = unclean_image();
        let upper = Ext4Upper::open(&path, "/").expect("open");
        assert!(upper.read_file("nope").is_err());
        assert!(upper.read_link("nope").is_err());
    }

    #[test]
    fn an_image_that_is_not_there_names_itself() {
        let err = Ext4Upper::open_run_upper(std::path::Path::new("/nonexistent/upper.img"))
            .err()
            .expect("a missing image must error");
        assert!(
            format!("{err:#}").contains("opening the run's upper volume /nonexistent/upper.img"),
            "{err:#}"
        );
    }

    #[test]
    fn an_image_that_is_not_ext4_names_the_volume() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        std::fs::write(&path, vec![0u8; 8192]).expect("write");
        let err = Ext4Upper::open_run_upper(&path)
            .err()
            .expect("garbage must not load");
        assert!(
            format!("{err:#}").contains("reading the run's upper volume"),
            "{err:#}"
        );
    }

    #[test]
    fn a_read_that_does_not_reach_the_feature_word_is_left_alone() {
        let mut before = [0xffu8; 4];
        clear_recovery_flag(SUPERBLOCK_START + FEATURE_INCOMPAT_OFFSET + 4, &mut before);
        assert_eq!(
            before, [0xff; 4],
            "a read past the word must not be touched"
        );

        let mut short = [0xffu8; 2];
        clear_recovery_flag(SUPERBLOCK_START + FEATURE_INCOMPAT_OFFSET, &mut short);
        assert_eq!(
            short, [0xff; 2],
            "a read that ends inside the word is left as read"
        );

        let mut early = [0xffu8; 4];
        clear_recovery_flag(0, &mut early);
        assert_eq!(
            early, [0xff; 4],
            "a read before the superblock must not be touched"
        );
    }

    #[test]
    fn every_entry_kind_the_upper_can_hold_maps_to_a_change_or_is_dropped() {
        assert_eq!(kind_of(FileType::Directory), UpperKind::Directory);
        assert_eq!(kind_of(FileType::Regular), UpperKind::Regular);
        assert_eq!(kind_of(FileType::Symlink), UpperKind::Symlink);
        assert_eq!(kind_of(FileType::CharacterDevice), UpperKind::Whiteout);
        assert_eq!(kind_of(FileType::BlockDevice), UpperKind::Unsupported);
        assert_eq!(kind_of(FileType::Fifo), UpperKind::Unsupported);
        assert_eq!(kind_of(FileType::Socket), UpperKind::Unsupported);
    }
}
