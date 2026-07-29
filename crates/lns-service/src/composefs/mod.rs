use std::fmt;

use sha2::{Sha256, digest::Output};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

pub(crate) mod changeset;
pub mod descriptor;
pub(crate) mod inject_path;
pub mod oci;
pub mod vendor;

pub use fsverity::{Algorithm, FsVerityHashValue};
pub use tree::{Directory, FileSystem, Inode, Leaf, RegularFile, Stat};
pub use vendor::{fsverity, tree};

#[derive(Clone, Eq, FromBytes, Hash, Immutable, IntoBytes, KnownLayout, PartialEq, Unaligned)]
#[repr(C)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<[u8; 32]> for Sha256Digest {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl From<Output<Sha256>> for Sha256Digest {
    fn from(value: Output<Sha256>) -> Self {
        Self(value.into())
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", Self::ALGORITHM.hash_name(), self.to_hex())
    }
}

impl FsVerityHashValue for Sha256Digest {
    type Digest = Sha256;
    const ALGORITHM: Algorithm = Algorithm::SHA256;
    const EMPTY: Self = Self([0; 32]);

    fn to_object_pathname(&self) -> String {
        format!("sha256/{}", self.to_hex())
    }
}

pub fn mkfs_erofs(fs: &tree::FileSystem<Sha256Digest>) -> Box<[u8]> {
    vendor::erofs::writer::mkfs_erofs(fs)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, ffi::OsStr};

    use super::*;
    use vendor::{generic_tree::LeafContent, tree::RegularFile};

    fn stat(mode: u32) -> Stat {
        Stat {
            st_mode: mode,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 0,
            xattrs: BTreeMap::new(),
        }
    }

    #[test]
    fn sha256_digest_uses_lns_content_store_pathname() {
        let digest = Sha256Digest::from([0xab; 32]);

        assert_eq!(
            digest.to_object_pathname(),
            format!("sha256/{}", "ab".repeat(32))
        );
    }

    #[test]
    fn sha256_digest_new_wraps_bytes_verbatim() {
        let bytes = [0x77u8; 32];
        let via_new = Sha256Digest::new(bytes);
        let via_from = Sha256Digest::from(bytes);
        assert_eq!(via_new, via_from);
    }

    #[test]
    fn sha256_digest_from_sha2_output_round_trips_through_hex() {
        use sha2::Digest as _;
        let payload = b"composefs";
        let raw = Sha256::digest(payload);
        let digest = Sha256Digest::from(raw);
        let expected_hex = hex::encode(raw);
        assert_eq!(
            digest.to_object_pathname(),
            format!("sha256/{expected_hex}")
        );
    }

    #[test]
    fn sha256_digest_debug_renders_algorithm_prefix_and_hex() {
        let digest = Sha256Digest::from([0xcd; 32]);
        assert_eq!(format!("{digest:?}"), format!("sha256:{}", "cd".repeat(32)));
    }

    #[test]
    fn mkfs_erofs_smoke_test() {
        let mut fs = FileSystem::<Sha256Digest>::new(stat(0o755));

        let file_id = fs.push_leaf(
            stat(0o644),
            LeafContent::Regular(RegularFile::External(Sha256Digest::from([0x42; 32]), 128)),
        );
        fs.root.insert(OsStr::new("file.txt"), Inode::leaf(file_id));

        let symlink_id = fs.push_leaf(
            stat(0o777),
            LeafContent::Symlink(OsStr::new("file.txt").into()),
        );
        fs.root
            .insert(OsStr::new("file-link"), Inode::leaf(symlink_id));

        assert_eq!(fs.nlinks(), vec![1, 1]);

        let image = mkfs_erofs(&fs);
        assert_eq!(&image[1024..1028], &[0xe2, 0xe1, 0xf5, 0xe0]);
    }
}
