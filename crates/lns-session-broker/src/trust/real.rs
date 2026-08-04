#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::os::unix::fs::PermissionsExt;

use super::TrustFs;

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
pub fn seed_trust_store() -> Result<(), String> {
    super::seed_trust_store_with(&RealTrustFs)
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
}
