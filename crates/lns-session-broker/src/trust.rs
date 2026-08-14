#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

//! The guest's trust store: seeded from the staged bundle when the rootfs ships none, then extended with the run's MITM CA so a sidecar's clients trust the proxy without the image cooperating.

mod real;
#[cfg(target_os = "linux")]
pub use real::{RealTrustStore, seed_trust_store};

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

pub trait TrustStore {
    fn read(&self, path: &str) -> std::io::Result<String>;
    fn append(&self, path: &str, pem: &str) -> std::io::Result<()>;
}

/// Take the CA out of `env`, dropped whether or not the install succeeds so no child can re-export it.
pub fn take_proxy_ca(env: &mut Vec<String>) -> Option<String> {
    let prefix = format!("{}=", lns_session::PROXY_CA_ENV);
    let mut pem = None;
    env.retain(|entry| match entry.strip_prefix(&prefix) {
        Some(value) => {
            if pem.is_none() && !value.is_empty() {
                pem = Some(value.to_string());
            }
            false
        }
        None => true,
    });
    pem
}

/// Append `pem` to the bundle unless it is already there, so a reconnect does not grow the file.
pub fn install(store: &dyn TrustStore, path: &str, pem: &str) -> Result<(), String> {
    let existing = store
        .read(path)
        .map_err(|e| format!("reading trust store {path}: {e}"))?;
    if existing.contains(pem.trim()) {
        return Ok(());
    }
    store
        .append(path, pem)
        .map_err(|e| format!("appending the proxy CA to {path}: {e}"))
}

/// Install the CA named in `env` if there is one; `None` is the ordinary case for a workload whose supervisor owns its own.
pub fn install_from_env(
    env: &mut Vec<String>,
    store: &dyn TrustStore,
) -> Option<Result<(), String>> {
    let pem = take_proxy_ca(env)?;
    Some(install(store, SYSTEM_BUNDLE, &pem))
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

    #[derive(Default)]
    struct FakeStore {
        contents: RefCell<String>,
        read_fails: bool,
        append_fails: bool,
    }

    impl TrustStore for FakeStore {
        fn read(&self, _path: &str) -> std::io::Result<String> {
            if self.read_fails {
                return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
            }
            Ok(self.contents.borrow().clone())
        }

        fn append(&self, _path: &str, pem: &str) -> std::io::Result<()> {
            if self.append_fails {
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            self.contents.borrow_mut().push_str(pem);
            Ok(())
        }
    }

    fn env_with_ca(pem: &str) -> Vec<String> {
        vec![
            "PATH=/usr/bin".to_string(),
            format!("{}={pem}", lns_session::PROXY_CA_ENV),
            "HOME=/root".to_string(),
        ]
    }

    #[test]
    fn the_ca_is_taken_out_of_the_env_the_workload_will_inherit() {
        // The workload has no use for it and a child re-exporting it is pure leak.
        let mut env = env_with_ca("PEM-BODY");
        assert_eq!(take_proxy_ca(&mut env).as_deref(), Some("PEM-BODY"));
        assert_eq!(
            env,
            vec!["PATH=/usr/bin".to_string(), "HOME=/root".to_string()]
        );
    }

    #[test]
    fn a_pem_carrying_base64_padding_survives_intact() {
        // PEM bodies end in '=' padding, so splitting on every '=' would truncate the cert.
        let pem = "-----BEGIN CERTIFICATE-----\nAAAB==\n-----END CERTIFICATE-----";
        let mut env = env_with_ca(pem);
        assert_eq!(take_proxy_ca(&mut env).as_deref(), Some(pem));
    }

    #[test]
    fn no_ca_in_the_env_leaves_it_untouched() {
        let mut env = vec!["PATH=/usr/bin".to_string()];
        assert!(take_proxy_ca(&mut env).is_none());
        assert_eq!(env, vec!["PATH=/usr/bin".to_string()]);
    }

    #[test]
    fn an_empty_value_is_no_ca_but_is_still_dropped_from_the_env() {
        let mut env = env_with_ca("");
        assert!(take_proxy_ca(&mut env).is_none());
        assert_eq!(
            env,
            vec!["PATH=/usr/bin".to_string(), "HOME=/root".to_string()]
        );
    }

    #[test]
    fn the_first_of_several_values_wins_and_every_copy_is_dropped() {
        let mut env = vec![
            format!("{}=first", lns_session::PROXY_CA_ENV),
            format!("{}=second", lns_session::PROXY_CA_ENV),
        ];
        assert_eq!(take_proxy_ca(&mut env).as_deref(), Some("first"));
        assert!(env.is_empty());
    }

    #[test]
    fn installing_appends_the_ca_to_the_bundle() {
        let store = FakeStore {
            contents: RefCell::new("EXISTING-ROOT".into()),
            ..FakeStore::default()
        };
        install(&store, SYSTEM_BUNDLE, "PEM-BODY").expect("install");
        assert_eq!(*store.contents.borrow(), "EXISTING-ROOTPEM-BODY");
    }

    #[test]
    fn installing_the_same_ca_twice_does_not_grow_the_bundle() {
        let store = FakeStore::default();
        install(&store, SYSTEM_BUNDLE, "PEM-BODY").expect("first install");
        install(&store, SYSTEM_BUNDLE, "PEM-BODY").expect("second install");
        assert_eq!(*store.contents.borrow(), "PEM-BODY");
    }

    #[test]
    fn an_image_without_a_trust_store_reports_the_path_it_looked_for() {
        // A scratch image has nothing to append to, so the caller logs and carries on.
        let store = FakeStore {
            read_fails: true,
            ..FakeStore::default()
        };
        let err = install(&store, SYSTEM_BUNDLE, "PEM-BODY").expect_err("no bundle");
        assert!(err.contains(SYSTEM_BUNDLE), "got: {err}");
    }

    #[test]
    fn an_unwritable_trust_store_reports_the_path_it_failed_on() {
        let store = FakeStore {
            append_fails: true,
            ..FakeStore::default()
        };
        let err = install(&store, SYSTEM_BUNDLE, "PEM-BODY").expect_err("read-only bundle");
        assert!(err.contains(SYSTEM_BUNDLE), "got: {err}");
    }

    #[test]
    fn install_from_env_installs_and_strips_in_one_step() {
        let store = FakeStore::default();
        let mut env = env_with_ca("PEM-BODY");
        install_from_env(&mut env, &store)
            .expect("a CA was present")
            .expect("install");
        assert_eq!(*store.contents.borrow(), "PEM-BODY");
        assert!(!env.iter().any(|e| e.contains(lns_session::PROXY_CA_ENV)));
    }

    #[test]
    fn install_from_env_does_nothing_when_the_run_sent_no_ca() {
        let store = FakeStore::default();
        let mut env = vec!["PATH=/usr/bin".to_string()];
        assert!(install_from_env(&mut env, &store).is_none());
        assert!(store.contents.borrow().is_empty());
    }
}
