#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

mod real;
#[cfg(target_os = "linux")]
pub use real::seed_trust_store;

use lns_session::{STAGED_CA_BUNDLE_PATH as STAGED_PEM, SYSTEM_CA_BUNDLE_PATH as SYSTEM_BUNDLE};

pub const CERTS_DIR: &str = "/etc/ssl/certs";

pub trait TrustFs {
    fn exists(&self, path: &str) -> bool;
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>>;
    fn create_dir_all(&self, path: &str) -> std::io::Result<()>;
    fn write(&self, path: &str, contents: &[u8], mode: u32) -> std::io::Result<()>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Seeding {
    ImageStoreKept,
    StagedStoreCopied,
    NoRootsAtAll,
}

impl Seeding {
    /// Only the broker knows both that the rootfs ships no store and that none was staged.
    pub fn report(&self) -> Option<String> {
        matches!(self, Self::NoRootsAtAll).then(|| {
            format!(
                "no trust store at {SYSTEM_BUNDLE} and none staged at {STAGED_PEM}; TLS from this workload will fail"
            )
        })
    }
}

/// Every workload process is handed `SSL_CERT_FILE` and friends naming the canonical bundle, and the proxy CA is appended to that same file — so a rootfs that ships none gets the staged store, and one that ships its own keeps every root it trusts.
pub fn seed_trust_store_with(fs: &dyn TrustFs) -> Result<Seeding, String> {
    if fs.exists(SYSTEM_BUNDLE) {
        return Ok(Seeding::ImageStoreKept);
    }
    let pem = match fs.read(STAGED_PEM) {
        Ok(pem) => pem,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Seeding::NoRootsAtAll),
        Err(e) => return Err(format!("reading {STAGED_PEM}: {e}")),
    };
    fs.create_dir_all(CERTS_DIR)
        .map_err(|e| format!("creating {CERTS_DIR}: {e}"))?;
    fs.write(SYSTEM_BUNDLE, &pem, 0o644)
        .map_err(|e| format!("writing {SYSTEM_BUNDLE}: {e}"))?;
    Ok(Seeding::StagedStoreCopied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io;

    #[derive(Default)]
    struct FakeTrustFs {
        present: Vec<&'static str>,
        staged: Option<Vec<u8>>,
        read_error: Option<io::ErrorKind>,
        fail_create_dir_all: bool,
        fail_write: bool,
        writes: RefCell<Vec<(String, Vec<u8>, u32)>>,
        dirs: RefCell<Vec<String>>,
    }

    impl FakeTrustFs {
        fn staged(pem: &[u8]) -> Self {
            Self {
                staged: Some(pem.to_vec()),
                ..Default::default()
            }
        }
    }

    impl TrustFs for FakeTrustFs {
        fn exists(&self, path: &str) -> bool {
            self.present.contains(&path)
        }
        fn read(&self, _path: &str) -> io::Result<Vec<u8>> {
            if let Some(kind) = self.read_error {
                return Err(io::Error::from(kind));
            }
            self.staged
                .clone()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
        fn create_dir_all(&self, path: &str) -> io::Result<()> {
            if self.fail_create_dir_all {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.dirs.borrow_mut().push(path.to_string());
            Ok(())
        }
        fn write(&self, path: &str, contents: &[u8], mode: u32) -> io::Result<()> {
            if self.fail_write {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.writes
                .borrow_mut()
                .push((path.to_string(), contents.to_vec(), mode));
            Ok(())
        }
    }

    #[test]
    fn a_rootfs_without_a_trust_store_is_seeded_from_the_staged_one() {
        let fs = FakeTrustFs::staged(b"PEM");
        assert_eq!(
            seed_trust_store_with(&fs).expect("seeded"),
            Seeding::StagedStoreCopied
        );
        assert_eq!(fs.dirs.borrow().as_slice(), [CERTS_DIR.to_string()]);
        assert_eq!(
            fs.writes.borrow().as_slice(),
            [(SYSTEM_BUNDLE.to_string(), b"PEM".to_vec(), 0o644)]
        );
    }

    #[test]
    fn a_trust_store_the_image_ships_is_left_untouched() {
        let fs = FakeTrustFs {
            present: vec![SYSTEM_BUNDLE],
            ..FakeTrustFs::staged(b"PEM")
        };
        assert_eq!(
            seed_trust_store_with(&fs).expect("no-op"),
            Seeding::ImageStoreKept
        );
        assert!(
            fs.writes.borrow().is_empty(),
            "overwriting the image's store would drop every root it added"
        );
    }

    #[test]
    fn a_guest_that_was_staged_no_store_boots_unchanged() {
        let fs = FakeTrustFs::default();
        assert_eq!(
            seed_trust_store_with(&fs).expect("no-op"),
            Seeding::NoRootsAtAll
        );
        assert!(fs.writes.borrow().is_empty());
    }

    #[test]
    fn a_guest_with_neither_a_shipped_nor_a_staged_store_is_named_as_having_no_roots() {
        let reported = Seeding::NoRootsAtAll.report().expect("a diagnostic");
        assert!(
            reported.contains(SYSTEM_BUNDLE) && reported.contains(STAGED_PEM),
            "the broker is the only place both facts are known, so both belong in the line: {reported}"
        );
    }

    #[test]
    fn the_two_states_that_leave_the_guest_with_roots_say_nothing() {
        assert_eq!(Seeding::ImageStoreKept.report(), None);
        assert_eq!(Seeding::StagedStoreCopied.report(), None);
    }

    #[test]
    fn an_unreadable_staged_store_is_reported() {
        let fs = FakeTrustFs {
            read_error: Some(io::ErrorKind::PermissionDenied),
            ..FakeTrustFs::staged(b"PEM")
        };
        let err = seed_trust_store_with(&fs).expect_err("reported");
        assert!(err.contains(STAGED_PEM), "got: {err}");
    }

    #[test]
    fn a_certs_dir_that_cannot_be_created_is_reported() {
        let fs = FakeTrustFs {
            fail_create_dir_all: true,
            ..FakeTrustFs::staged(b"PEM")
        };
        let err = seed_trust_store_with(&fs).expect_err("reported");
        assert!(err.contains(CERTS_DIR), "got: {err}");
    }

    #[test]
    fn a_store_that_cannot_be_written_is_reported() {
        let fs = FakeTrustFs {
            fail_write: true,
            ..FakeTrustFs::staged(b"PEM")
        };
        let err = seed_trust_store_with(&fs).expect_err("reported");
        assert!(err.contains(SYSTEM_BUNDLE), "got: {err}");
    }
}
