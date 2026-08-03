#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

use super::{TrustFs, TrustStore};

#[derive(Default)]
struct RealTrustFs;

impl TrustFs for RealTrustFs {
    /// `metadata`, so a link the image points at a real store counts as one and a link with nothing behind it does not — the latter holds no roots, and seeding writes through it to the target the image chose.
    fn exists(&self, path: &str) -> bool {
        std::fs::metadata(path).is_ok()
    }

    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn create_dir_all(&self, path: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn write(&self, path: &str, contents: &[u8], mode: u32) -> std::io::Result<()> {
        std::fs::write(path, contents)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
}

#[cfg(target_os = "linux")]
pub fn seed_trust_store() -> Result<super::Seeding, String> {
    super::seed_trust_store_with(&RealTrustFs)
}

pub struct RealTrustStore;

impl TrustStore for RealTrustStore {
    fn read(&self, path: &str) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }

    /// Leading newline so the CA starts its own PEM block even if the bundle did not end with one.
    fn append(&self, path: &str, pem: &str) -> std::io::Result<()> {
        let mut file = OpenOptions::new().append(true).open(path)?;
        writeln!(file, "\n{}", pem.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symlink_to_a_store_the_image_placed_counts_as_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("image-roots.pem");
        std::fs::write(&target, b"PEM").expect("target");
        let link = dir.path().join("ca-certificates.crt");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(RealTrustFs.exists(link.to_str().expect("utf-8 path")));
    }

    #[test]
    fn a_symlink_with_no_target_does_not_count_as_a_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let link = dir.path().join("ca-certificates.crt");
        std::os::unix::fs::symlink(dir.path().join("gone.pem"), &link).expect("symlink");
        assert!(
            !RealTrustFs.exists(link.to_str().expect("utf-8 path")),
            "a link with nothing behind it holds no roots, and the supervisor cannot append the proxy CA to it either"
        );
    }

    #[test]
    fn a_path_with_nothing_at_it_does_not_count_as_a_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("ca-certificates.crt");
        assert!(!RealTrustFs.exists(missing.to_str().expect("utf-8 path")));
    }

    #[test]
    fn a_written_store_lands_world_readable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("certs").join("ca-certificates.crt");
        let parent = path.parent().expect("parent").to_path_buf();
        RealTrustFs
            .create_dir_all(parent.to_str().expect("utf-8 path"))
            .expect("mkdir");
        let path_str = path.to_str().expect("utf-8 path");
        RealTrustFs.write(path_str, b"PEM", 0o644).expect("write");
        assert_eq!(RealTrustFs.read(path_str).expect("read back"), b"PEM");
        assert_eq!(
            std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn seeding_a_link_with_no_target_lands_the_store_where_the_image_pointed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("image-roots.pem");
        let link = dir.path().join("ca-certificates.crt");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        RealTrustFs
            .write(link.to_str().expect("utf-8 path"), b"PEM", 0o644)
            .expect("write");
        assert_eq!(std::fs::read(&target).expect("target"), b"PEM");
    }

    #[test]
    fn an_unreadable_path_surfaces_the_os_error() {
        let err = RealTrustFs
            .read("/does/not/exist/ca-certificates.crt")
            .expect_err("read should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn appending_leaves_both_the_existing_roots_and_the_ca_readable() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let bundle = dir.path().join("ca-certificates.crt");
        std::fs::write(&bundle, "EXISTING-ROOT").expect("seed");
        let path = bundle.to_str().expect("utf8 path");

        RealTrustStore.append(path, "PEM-BODY").expect("append");

        let got = RealTrustStore.read(path).expect("read");
        assert_eq!(got, "EXISTING-ROOT\nPEM-BODY\n");
    }

    #[test]
    fn appending_to_a_missing_bundle_is_an_error_not_a_new_file() {
        // A trust store we created from nothing would trust the proxy and nothing
        // else, breaking every unrelated TLS client in the guest.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let missing = dir.path().join("absent.crt");
        let path = missing.to_str().expect("utf8 path");

        assert!(RealTrustStore.append(path, "PEM-BODY").is_err());
        assert!(!missing.exists());
    }

    #[test]
    fn reading_a_missing_bundle_surfaces_the_io_error() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("absent.crt");
        assert!(
            RealTrustStore
                .read(path.to_str().expect("utf8 path"))
                .is_err()
        );
    }
}
