use crate::log;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

mod real;
mod traits;
pub(crate) use real::{RealFetcher, RealFs};
pub(crate) use traits::{Fetcher, Fs, WritableFile};

pub(crate) struct PinnedArtifact<'a> {
    pub filename: &'a str,
    pub url: &'a str,
    pub sha256: &'a str,
    pub mode: Option<u32>,
    pub label: &'a str,
}

pub(crate) struct VerifiedArtifacts(std::sync::Mutex<std::collections::BTreeSet<PathBuf>>);

impl VerifiedArtifacts {
    pub(crate) const fn new() -> Self {
        Self(std::sync::Mutex::new(std::collections::BTreeSet::new()))
    }

    fn contains(&self, path: &Path) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(path)
    }

    fn insert(&self, path: PathBuf) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path);
    }
}

// Re-hashing a ~50MB artifact costs ~60ms on every run; once this process has verified the pinned sha, a still-present file is trusted for the process lifetime.
pub(crate) async fn ensure_pinned_memoized(
    fetcher: &impl Fetcher,
    fs: &impl Fs,
    cache: &Path,
    artifact: &PinnedArtifact<'_>,
    memo: &VerifiedArtifacts,
) -> Result<PathBuf> {
    let path = cache.join(artifact.filename);
    if memo.contains(&path) && fs.is_file(&path).await {
        return Ok(path);
    }
    let resolved = ensure_pinned(fetcher, fs, cache, artifact).await?;
    memo.insert(resolved.clone());
    Ok(resolved)
}

#[allow(clippy::cognitive_complexity)] // cache hit/corrupt/miss are one resolution decision; extracting them buries the fall-through to fetch
pub(crate) async fn ensure_pinned(
    fetcher: &impl Fetcher,
    fs: &impl Fs,
    cache: &Path,
    artifact: &PinnedArtifact<'_>,
) -> Result<PathBuf> {
    fs.create_dir_all(cache)
        .await
        .with_context(|| format!("create_dir_all {}", cache.display()))?;
    let path = cache.join(artifact.filename);

    if fs.is_file(&path).await {
        match fs.read(&path).await {
            Ok(bytes) => {
                let actual = format!("{:x}", Sha256::digest(&bytes));
                if actual == artifact.sha256 {
                    return Ok(path);
                }
                let path_str = path.display();
                let label = artifact.label;
                let expected = artifact.sha256;
                log::warn!(
                    "cached {label} at {path_str} has wrong sha256 ({actual} vs expected {expected}) — re-downloading"
                );
            }
            Err(e) => {
                let path_str = path.display();
                let label = artifact.label;
                let cause = format!("{e:#}");
                log::warn!("cached {label} at {path_str} is unreadable ({cause}) — re-downloading");
            }
        }
    }

    let label = artifact.label;
    log::info!("Fetching", "{label} (one-time)");
    log::debug!(url = %artifact.url, "downloading artifact");
    let bytes = fetcher.fetch(artifact.url).await?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != artifact.sha256 {
        bail!(
            "{label} sha256 mismatch — expected {}, got {actual} for {}. Either the \
             published artifact was rotated without a pin bump, or the download was \
             tampered. Refusing to install.",
            artifact.sha256,
            artifact.url
        );
    }
    atomic_write(fs, &path, &bytes, artifact.mode)
        .await
        .with_context(|| format!("installing {}", path.display()))?;
    Ok(path)
}

async fn atomic_write(fs: &impl Fs, path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs.remove_file(&tmp).await;
    {
        let mut f = fs
            .create_new(&tmp)
            .await
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(bytes)
            .await
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.sync_all()
            .await
            .with_context(|| format!("fsync {}", tmp.display()))?;
    }
    if let Some(m) = mode {
        fs.set_mode(&tmp, m)
            .await
            .with_context(|| format!("chmod {m:o} {}", tmp.display()))?;
    }
    fs.rename(&tmp, path)
        .await
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    const TEST_URL: &str = "https://test.cdn.invalid/artifact";

    fn payload() -> Vec<u8> {
        b"test artifact bytes".to_vec()
    }

    fn payload_sha() -> String {
        format!("{:x}", Sha256::digest(payload()))
    }

    fn art<'a>(sha: &'a str, mode: Option<u32>) -> PinnedArtifact<'a> {
        PinnedArtifact {
            filename: "artifact.bin",
            url: TEST_URL,
            sha256: sha,
            mode,
            label: "test artifact",
        }
    }

    fn init_tracing_capture() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .finish();
            tracing::subscriber::set_global_default(subscriber).ok();
        });
    }

    struct CannedFetcher {
        plan: std::sync::Mutex<std::collections::VecDeque<Result<Vec<u8>>>>,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl CannedFetcher {
        fn never() -> Self {
            Self {
                plan: std::sync::Mutex::new(std::collections::VecDeque::new()),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn ok_once(bytes: Vec<u8>) -> Self {
            let mut plan = std::collections::VecDeque::new();
            plan.push_back(Ok(bytes));
            Self {
                plan: std::sync::Mutex::new(plan),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn err_once(e: anyhow::Error) -> Self {
            let mut plan = std::collections::VecDeque::new();
            plan.push_back(Err(e));
            Self {
                plan: std::sync::Mutex::new(plan),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Fetcher for CannedFetcher {
        async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
            self.calls.lock().unwrap().push(url.to_string());
            self.plan
                .lock()
                .unwrap()
                .pop_front()
                .expect("CannedFetcher: fetch called more times than planned")
        }
    }

    #[derive(Default)]
    struct FakeState {
        files: std::collections::HashMap<PathBuf, Vec<u8>>,
        modes: std::collections::HashMap<PathBuf, u32>,
        fail_create_dir_all: Option<io::Error>,
        fail_read: Option<io::Error>,
        fail_remove_file: Option<io::Error>,
        fail_create_new: Option<io::Error>,
        fail_write_all: Option<io::Error>,
        fail_sync_all: Option<io::Error>,
        fail_set_mode: Option<io::Error>,
        fail_rename: Option<io::Error>,
    }

    #[derive(Clone)]
    struct FakeFs {
        state: std::sync::Arc<std::sync::Mutex<FakeState>>,
    }

    impl FakeFs {
        fn new() -> Self {
            Self {
                state: std::sync::Arc::new(std::sync::Mutex::new(FakeState::default())),
            }
        }
        fn put_file(&self, p: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) {
            self.state
                .lock()
                .unwrap()
                .files
                .insert(p.into(), bytes.into());
        }
        fn read_file(&self, p: &Path) -> Option<Vec<u8>> {
            self.state.lock().unwrap().files.get(p).cloned()
        }
        fn file_exists(&self, p: &Path) -> bool {
            self.state.lock().unwrap().files.contains_key(p)
        }
        fn mode_of(&self, p: &Path) -> Option<u32> {
            self.state.lock().unwrap().modes.get(p).copied()
        }
        fn fail_next_create_dir_all(&self, e: io::Error) {
            self.state.lock().unwrap().fail_create_dir_all = Some(e);
        }
        fn fail_next_read(&self, e: io::Error) {
            self.state.lock().unwrap().fail_read = Some(e);
        }
        fn fail_next_remove_file(&self, e: io::Error) {
            self.state.lock().unwrap().fail_remove_file = Some(e);
        }
        fn fail_next_create_new(&self, e: io::Error) {
            self.state.lock().unwrap().fail_create_new = Some(e);
        }
        fn fail_next_write_all(&self, e: io::Error) {
            self.state.lock().unwrap().fail_write_all = Some(e);
        }
        fn fail_next_sync_all(&self, e: io::Error) {
            self.state.lock().unwrap().fail_sync_all = Some(e);
        }
        fn fail_next_set_mode(&self, e: io::Error) {
            self.state.lock().unwrap().fail_set_mode = Some(e);
        }
        fn fail_next_rename(&self, e: io::Error) {
            self.state.lock().unwrap().fail_rename = Some(e);
        }
    }

    impl Fs for FakeFs {
        type WritableFile = FakeWritableFile;

        async fn create_dir_all(&self, _p: &Path) -> io::Result<()> {
            match self.state.lock().unwrap().fail_create_dir_all.take() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
        async fn is_file(&self, p: &Path) -> bool {
            self.state.lock().unwrap().files.contains_key(p)
        }
        async fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_read.take() {
                return Err(e);
            }
            let entry = s.files.get(p).cloned();
            Ok(entry.unwrap_or_else(|| panic!("FakeFs::read: no fixture for {}", p.display())))
        }
        async fn remove_file(&self, p: &Path) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_remove_file.take() {
                return Err(e);
            }
            s.files.remove(p);
            Ok(())
        }
        async fn create_new(&self, p: &Path) -> io::Result<FakeWritableFile> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_create_new.take() {
                return Err(e);
            }
            let path_str = p.display();
            assert!(
                !s.files.contains_key(p),
                "FakeFs::create_new: {path_str} already exists"
            );
            s.files.insert(p.to_path_buf(), Vec::new());
            Ok(FakeWritableFile {
                path: p.to_path_buf(),
                state: std::sync::Arc::clone(&self.state),
            })
        }
        async fn set_mode(&self, p: &Path, mode: u32) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_set_mode.take() {
                return Err(e);
            }
            s.modes.insert(p.to_path_buf(), mode);
            Ok(())
        }
        async fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_rename.take() {
                return Err(e);
            }
            let bytes = s
                .files
                .remove(from)
                .unwrap_or_else(|| panic!("FakeFs::rename: no source at {}", from.display()));
            s.files.insert(to.to_path_buf(), bytes);
            if let Some(m) = s.modes.remove(from) {
                s.modes.insert(to.to_path_buf(), m);
            }
            Ok(())
        }
    }

    struct FakeWritableFile {
        path: PathBuf,
        state: std::sync::Arc<std::sync::Mutex<FakeState>>,
    }

    impl WritableFile for FakeWritableFile {
        async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_write_all.take() {
                return Err(e);
            }
            s.files
                .get_mut(&self.path)
                .expect("fake fs: write_all on a path that was never create_new'd")
                .extend_from_slice(bytes);
            Ok(())
        }
        async fn sync_all(&mut self) -> io::Result<()> {
            if let Some(e) = self.state.lock().unwrap().fail_sync_all.take() {
                return Err(e);
            }
            Ok(())
        }
    }

    fn cache_root() -> PathBuf {
        PathBuf::from("/fake/cache/dl")
    }

    fn cache_file() -> PathBuf {
        cache_root().join("artifact.bin")
    }

    #[tokio::test]
    async fn verified_artifact_is_not_rehashed_on_the_next_ensure() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.put_file(cache_file(), payload());
        let memo = VerifiedArtifacts::new();

        ensure_pinned_memoized(
            &CannedFetcher::never(),
            &fs,
            &cache_root(),
            &art(&payload_sha(), None),
            &memo,
        )
        .await
        .expect("first ensure verifies the cached bytes");

        fs.fail_next_read(io::Error::new(io::ErrorKind::PermissionDenied, "reread"));
        let resolved = ensure_pinned_memoized(
            &CannedFetcher::never(),
            &fs,
            &cache_root(),
            &art(&payload_sha(), None),
            &memo,
        )
        .await
        .expect("second ensure must trust the memo instead of re-reading");
        assert_eq!(resolved, cache_file());
    }

    #[tokio::test]
    async fn memo_hit_with_deleted_file_falls_through_to_fetch() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.put_file(cache_file(), payload());
        let memo = VerifiedArtifacts::new();

        ensure_pinned_memoized(
            &CannedFetcher::never(),
            &fs,
            &cache_root(),
            &art(&payload_sha(), None),
            &memo,
        )
        .await
        .expect("first ensure verifies");

        let fs2 = FakeFs::new();
        let fetcher = CannedFetcher::ok_once(payload());
        let resolved =
            ensure_pinned_memoized(&fetcher, &fs2, &cache_root(), &art(&payload_sha(), None), &memo)
                .await
                .expect("deleted artifact must self-heal via fetch");
        assert_eq!(resolved, cache_file());
        assert_eq!(fetcher.calls().len(), 1, "gone-from-disk memo hit must fetch");
    }

    #[tokio::test]
    async fn fresh_download_populates_the_memo() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let memo = VerifiedArtifacts::new();

        ensure_pinned_memoized(
            &CannedFetcher::ok_once(payload()),
            &fs,
            &cache_root(),
            &art(&payload_sha(), None),
            &memo,
        )
        .await
        .expect("download installs");

        fs.fail_next_read(io::Error::new(io::ErrorKind::PermissionDenied, "reread"));
        ensure_pinned_memoized(
            &CannedFetcher::never(),
            &fs,
            &cache_root(),
            &art(&payload_sha(), None),
            &memo,
        )
        .await
        .expect("freshly installed artifact must be memoized as verified");
    }

    #[tokio::test]
    async fn cached_artifact_with_matching_sha_returns_path_without_fetching() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.put_file(cache_file(), payload());
        let fetcher = CannedFetcher::never();

        let resolved = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect("cache hit should succeed");

        assert_eq!(resolved, cache_file());
        assert!(
            fetcher.calls().is_empty(),
            "cache hit must not fetch: {:?}",
            fetcher.calls()
        );
    }

    #[tokio::test]
    async fn cached_artifact_with_wrong_sha_redownloads() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.put_file(cache_file(), b"corrupted".to_vec());
        let fetcher = CannedFetcher::ok_once(payload());

        let resolved = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect("wrong-sha cache should self-heal");

        assert_eq!(resolved, cache_file());
        assert_eq!(fetcher.calls().len(), 1, "wrong-sha cache must fetch once");
        assert_eq!(fs.read_file(&cache_file()), Some(payload()));
    }

    #[tokio::test]
    async fn cached_artifact_unreadable_falls_through_to_fetcher() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.put_file(cache_file(), b"junk".to_vec());
        fs.fail_next_read(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
        let fetcher = CannedFetcher::ok_once(payload());

        let resolved = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect("unreadable cache must self-heal");

        assert_eq!(resolved, cache_file());
        assert_eq!(fetcher.calls().len(), 1);
        assert_eq!(fs.read_file(&cache_file()), Some(payload()));
    }

    #[tokio::test]
    async fn fetched_bytes_with_correct_sha_land_at_canonical_path() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let fetcher = CannedFetcher::ok_once(payload());

        let resolved = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect("happy path");

        assert_eq!(resolved, cache_file());
        assert_eq!(fs.read_file(&cache_file()), Some(payload()));
        assert!(
            !fs.file_exists(&cache_file().with_extension("tmp")),
            "tmp must not remain after success"
        );
    }

    #[tokio::test]
    async fn executable_mode_is_set_on_the_installed_binary() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let fetcher = CannedFetcher::ok_once(payload());

        let resolved = ensure_pinned(
            &fetcher,
            &fs,
            &cache_root(),
            &art(&payload_sha(), Some(0o755)),
        )
        .await
        .expect("happy path with exec bit");

        assert_eq!(
            fs.mode_of(&resolved),
            Some(0o755),
            "a VMM binary must be installed executable"
        );
    }

    #[tokio::test]
    async fn no_mode_leaves_the_file_unchmodded() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let fetcher = CannedFetcher::ok_once(payload());

        let resolved = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect("happy path");

        assert_eq!(
            fs.mode_of(&resolved),
            None,
            "the kernel path passes no mode, so set_mode is never called"
        );
    }

    #[tokio::test]
    async fn fetched_bytes_with_wrong_sha_bail_before_atomic_write() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let fetcher = CannedFetcher::ok_once(b"tampered".to_vec());

        let err = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect_err("sha mismatch must bail");

        let msg = format!("{err:#}");
        assert!(msg.contains("sha256 mismatch"), "category: {msg}");
        assert!(msg.contains("Refusing to install"), "verb: {msg}");
        assert!(msg.contains(TEST_URL), "url: {msg}");
        assert!(
            !fs.file_exists(&cache_file()),
            "tampered bytes must not land"
        );
        assert!(!fs.file_exists(&cache_file().with_extension("tmp")));
    }

    #[tokio::test]
    async fn fetcher_error_propagates() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let fetcher = CannedFetcher::err_once(anyhow::anyhow!("simulated transport error"));

        let err = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect_err("fetcher failure must propagate");

        assert!(format!("{err:#}").contains("simulated transport error"));
        assert!(!fs.file_exists(&cache_file()));
    }

    #[tokio::test]
    async fn create_dir_all_failure_is_surfaced_with_context() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.fail_next_create_dir_all(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
        let fetcher = CannedFetcher::never();

        let err = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect_err("mkdir failure must bail before fetch");

        let msg = format!("{err:#}");
        assert!(msg.contains("create_dir_all"), "context: {msg}");
        assert!(
            fetcher.calls().is_empty(),
            "mkdir failure must bail before fetch"
        );
    }

    #[tokio::test]
    async fn pre_existing_tmp_remove_failure_is_silent() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.fail_next_remove_file(io::Error::new(io::ErrorKind::PermissionDenied, "stale"));
        let fetcher = CannedFetcher::ok_once(payload());

        let resolved = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect("remove_file failure must be discarded silently");

        assert_eq!(resolved, cache_file());
        assert_eq!(fs.read_file(&cache_file()), Some(payload()));
    }

    #[tokio::test]
    async fn create_new_failure_surfaces_with_path() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.fail_next_create_new(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
        let fetcher = CannedFetcher::ok_once(payload());

        let err = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect_err("create_new failure must propagate");

        let msg = format!("{err:#}");
        let tmp = cache_file().with_extension("tmp");
        assert!(msg.contains("installing"), "outer: {msg}");
        assert!(msg.contains("creating"), "inner: {msg}");
        assert!(msg.contains(tmp.to_str().unwrap()), "tmp path: {msg}");
        assert!(!fs.file_exists(&cache_file()));
    }

    #[tokio::test]
    async fn write_all_failure_surfaces_with_tmp_path() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.fail_next_write_all(io::Error::other("disk full"));
        let fetcher = CannedFetcher::ok_once(payload());

        let err = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect_err("write_all failure must propagate");

        let msg = format!("{err:#}");
        assert!(msg.contains("writing"), "context: {msg}");
        assert!(!fs.file_exists(&cache_file()));
    }

    #[tokio::test]
    async fn sync_all_failure_surfaces_with_tmp_path() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.fail_next_sync_all(io::Error::other("fsync failed"));
        let fetcher = CannedFetcher::ok_once(payload());

        let err = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect_err("sync_all failure must propagate");

        assert!(format!("{err:#}").contains("fsync"), "context");
        assert!(!fs.file_exists(&cache_file()));
    }

    #[tokio::test]
    async fn set_mode_failure_surfaces_with_chmod_context() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.fail_next_set_mode(io::Error::other("chmod denied"));
        let fetcher = CannedFetcher::ok_once(payload());

        let err = ensure_pinned(
            &fetcher,
            &fs,
            &cache_root(),
            &art(&payload_sha(), Some(0o755)),
        )
        .await
        .expect_err("set_mode failure must propagate");

        let msg = format!("{err:#}");
        assert!(msg.contains("chmod"), "context: {msg}");
        assert!(
            !fs.file_exists(&cache_file()),
            "a chmod failure must not leave the binary at its final path"
        );
    }

    #[tokio::test]
    async fn rename_failure_surfaces_with_both_paths() {
        init_tracing_capture();
        let fs = FakeFs::new();
        fs.fail_next_rename(io::Error::other("rename failed"));
        let fetcher = CannedFetcher::ok_once(payload());

        let err = ensure_pinned(&fetcher, &fs, &cache_root(), &art(&payload_sha(), None))
            .await
            .expect_err("rename failure must propagate");

        let msg = format!("{err:#}");
        let tmp = cache_file().with_extension("tmp");
        assert!(msg.contains("rename"), "context: {msg}");
        assert!(msg.contains(tmp.to_str().unwrap()), "tmp: {msg}");
        assert!(msg.contains(cache_file().to_str().unwrap()), "dest: {msg}");
        assert!(!fs.file_exists(&cache_file()));
    }
}
