use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

use crate::download::{Fetcher, Fs, PinnedArtifact, ensure_pinned};
use crate::log;
use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};
use crate::tools::{Arch, mise, provisioner};

pub(crate) async fn ensure_pem<F: Fetcher, S: Fs>(
    fetcher: &F,
    fs: &S,
    manifest: &mise::Manifest,
    cache_dir: &Path,
    arch: Arch,
) -> Result<PathBuf> {
    ensure_pinned(
        fetcher,
        fs,
        &provisioner::ca_cache_dir(cache_dir, arch),
        &PinnedArtifact {
            filename: &format!("cacert-{}.pem", manifest.ca_bundle.date),
            url: &manifest.ca_bundle.url(),
            sha256: &manifest.ca_bundle.sha256,
            mode: Some(0o644),
            label: "CA bundle",
        },
    )
    .await
}

/// Generous for a ~230KB PEM, and the bound belongs here rather than on the shared fetcher, which also carries the kernel and VMM transfers.
const FETCH_BUDGET: Duration = Duration::from_secs(10);

/// Every workload process inherits `SSL_CERT_FILE` and friends naming the canonical bundle path, and the proxy CA is appended to that same file — so a run gets the pinned store staged even when nothing else needs it, and a run that cannot have it keeps booting on whatever the image ships.
pub(crate) async fn workload_spec<F: Fetcher, S: Fs>(
    fetcher: &F,
    fs: &S,
    manifest: &mise::Manifest,
    cache_dir: &Path,
    arch: Arch,
) -> Option<RuntimeFileSpec> {
    workload_spec_within(fetcher, fs, manifest, cache_dir, arch, FETCH_BUDGET).await
}

async fn workload_spec_within<F: Fetcher, S: Fs>(
    fetcher: &F,
    fs: &S,
    manifest: &mise::Manifest,
    cache_dir: &Path,
    arch: Arch,
    budget: Duration,
) -> Option<RuntimeFileSpec> {
    let ensured = tokio::time::timeout(budget, ensure_pem(fetcher, fs, manifest, cache_dir, arch))
        .await
        .unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "the pinned store did not arrive within {budget:?}"
            ))
        });
    match ensured {
        Ok(path) => Some(RuntimeFileSpec {
            guest_path: lns_session::STAGED_CA_BUNDLE_PATH.to_string(),
            mode: 0o644,
            source: RuntimeSource::HostFile(path),
        }),
        Err(e) => {
            log::warn!(
                "could not stage the pinned CA store for the workload ({e:#}); the guest keeps whatever its image ships"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::WritableFile;
    use std::collections::HashMap;
    use std::io;
    use std::sync::{Arc, Mutex};

    const PEM: &[u8] = b"-----BEGIN CERTIFICATE-----\n";

    fn manifest(sha256: &str) -> mise::Manifest {
        toml::from_str(&format!(
            r#"
            [engine]
            version = "2026.7.14"
            [engine.sha256]
            aarch64 = "a"
            x86_64 = "b"
            [provisioner_rootfs.gnu]
            aarch64 = "debian@sha256:a"
            x86_64 = "debian@sha256:b"
            [provisioner_rootfs.musl]
            aarch64 = "alpine@sha256:a"
            x86_64 = "alpine@sha256:b"
            [static_curl]
            version = "8.11.0"
            [static_curl.sha256]
            aarch64 = "a"
            x86_64 = "b"
            [ca_bundle]
            date = "2026-07-16"
            sha256 = "{sha256}"
            "#
        ))
        .expect("a usable manifest")
    }

    fn pem_sha256() -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(PEM))
    }

    #[derive(Default)]
    struct FakeFetcher {
        calls: Arc<Mutex<Vec<String>>>,
        reachable: bool,
        stalls: bool,
    }

    impl Fetcher for FakeFetcher {
        async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
            self.calls.lock().expect("calls mutex").push(url.into());
            if self.stalls {
                std::future::pending::<()>().await;
            }
            if self.reachable {
                Ok(PEM.to_vec())
            } else {
                anyhow::bail!("no route to {url}")
            }
        }
    }

    #[derive(Clone, Default)]
    struct FakeFs {
        files: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
        modes: Arc<Mutex<HashMap<PathBuf, u32>>>,
    }

    struct FakeWritableFile {
        path: PathBuf,
        files: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
    }

    impl WritableFile for FakeWritableFile {
        async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.files
                .lock()
                .expect("files mutex")
                .entry(self.path.clone())
                .or_default()
                .extend_from_slice(bytes);
            Ok(())
        }
        async fn sync_all(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Fs for FakeFs {
        type WritableFile = FakeWritableFile;

        async fn create_dir_all(&self, _p: &Path) -> io::Result<()> {
            Ok(())
        }
        async fn is_file(&self, p: &Path) -> bool {
            self.files.lock().expect("files mutex").contains_key(p)
        }
        async fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
            self.files
                .lock()
                .expect("files mutex")
                .get(p)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
        async fn remove_file(&self, p: &Path) -> io::Result<()> {
            self.files.lock().expect("files mutex").remove(p);
            Ok(())
        }
        async fn create_new(&self, p: &Path) -> io::Result<Self::WritableFile> {
            Ok(FakeWritableFile {
                path: p.to_path_buf(),
                files: self.files.clone(),
            })
        }
        async fn set_mode(&self, p: &Path, mode: u32) -> io::Result<()> {
            self.modes
                .lock()
                .expect("modes mutex")
                .insert(p.to_path_buf(), mode);
            Ok(())
        }
        async fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            let mut files = self.files.lock().expect("files mutex");
            let bytes = files
                .remove(from)
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            files.insert(to.to_path_buf(), bytes);
            let mode = self.modes.lock().expect("modes mutex").remove(from);
            if let Some(mode) = mode {
                self.modes
                    .lock()
                    .expect("modes mutex")
                    .insert(to.to_path_buf(), mode);
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn the_workload_gets_the_pinned_store_at_the_path_the_broker_reads() {
        let fetcher = FakeFetcher {
            reachable: true,
            ..Default::default()
        };
        let fs = FakeFs::default();
        let spec = workload_spec(
            &fetcher,
            &fs,
            &manifest(&pem_sha256()),
            Path::new("/cache"),
            Arch::Aarch64,
        )
        .await
        .expect("a staged spec");
        let cached = provisioner::ca_cache_dir(Path::new("/cache"), Arch::Aarch64)
            .join("cacert-2026-07-16.pem");
        assert_eq!(spec.guest_path, lns_session::STAGED_CA_BUNDLE_PATH);
        assert_eq!(spec.mode, 0o644);
        assert!(
            matches!(&spec.source, RuntimeSource::HostFile(staged) if staged == &cached),
            "the store is staged from the cached file, not inlined: {spec:?}"
        );
        assert_eq!(
            fs.modes.lock().expect("modes mutex").get(&cached),
            Some(&0o644),
            "the guest reads the staged store as an unprivileged user"
        );
        assert_eq!(
            fetcher.calls.lock().expect("calls mutex").as_slice(),
            ["https://curl.se/ca/cacert-2026-07-16.pem"]
        );
    }

    #[tokio::test]
    async fn a_store_already_cached_by_a_provision_is_reused_without_a_fetch() {
        let fetcher = FakeFetcher::default();
        let fs = FakeFs::default();
        fs.files.lock().expect("files mutex").insert(
            provisioner::ca_cache_dir(Path::new("/cache"), Arch::X86_64)
                .join("cacert-2026-07-16.pem"),
            PEM.to_vec(),
        );
        assert!(
            workload_spec(
                &fetcher,
                &fs,
                &manifest(&pem_sha256()),
                Path::new("/cache"),
                Arch::X86_64,
            )
            .await
            .is_some()
        );
        assert!(
            fetcher.calls.lock().expect("calls mutex").is_empty(),
            "a warm cache must not put a network fetch in front of every run"
        );
    }

    #[tokio::test]
    async fn an_unreachable_pin_stages_nothing_and_lets_the_run_boot() {
        let spec = workload_spec(
            &FakeFetcher::default(),
            &FakeFs::default(),
            &manifest(&pem_sha256()),
            Path::new("/cache"),
            Arch::Aarch64,
        )
        .await;
        assert!(spec.is_none(), "a run must not fail over the CA store");
    }

    #[tokio::test(start_paused = true)]
    async fn a_fetch_that_never_answers_stops_holding_the_boot_and_stages_nothing() {
        let fetcher = FakeFetcher {
            reachable: true,
            stalls: true,
            ..Default::default()
        };
        let spec = workload_spec_within(
            &fetcher,
            &FakeFs::default(),
            &manifest(&pem_sha256()),
            Path::new("/cache"),
            Arch::Aarch64,
            Duration::from_secs(10),
        )
        .await;
        assert!(
            spec.is_none(),
            "a blackholed route must not hold the guest before it boots"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_warm_cache_never_waits_on_the_budget() {
        let fs = FakeFs::default();
        fs.files.lock().expect("files mutex").insert(
            provisioner::ca_cache_dir(Path::new("/cache"), Arch::Aarch64)
                .join("cacert-2026-07-16.pem"),
            PEM.to_vec(),
        );
        let started = tokio::time::Instant::now();
        assert!(
            workload_spec_within(
                &FakeFetcher::default(),
                &fs,
                &manifest(&pem_sha256()),
                Path::new("/cache"),
                Arch::Aarch64,
                Duration::from_secs(10),
            )
            .await
            .is_some()
        );
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[tokio::test]
    async fn a_rotated_store_that_no_longer_matches_the_pin_stages_nothing() {
        let fetcher = FakeFetcher {
            reachable: true,
            ..Default::default()
        };
        assert!(
            workload_spec(
                &fetcher,
                &FakeFs::default(),
                &manifest(&"a".repeat(64)),
                Path::new("/cache"),
                Arch::Aarch64,
            )
            .await
            .is_none(),
            "a trust store whose bytes are not the pinned ones must never reach a guest"
        );
    }
}
