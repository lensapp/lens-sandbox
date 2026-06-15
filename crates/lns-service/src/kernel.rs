use crate::log;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const KERNEL_VERSION: &str = env!("KERNEL_VERSION");

const KERNEL_SHA256: &str = env!("KERNEL_SHA256");

#[cfg(target_arch = "aarch64")]
const ARCH: &str = "aarch64";

#[cfg(target_arch = "x86_64")]
const ARCH: &str = "x86_64";

const CDN_BASE: &str = "https://get.lns.run";

pub(super) const MAX_KERNEL_BYTES: u64 = 64 * 1024 * 1024;

mod real;
mod traits;
use real::{RealFetcher, RealFs};
use traits::{Fetcher, Fs, WritableFile};

pub async fn ensure() -> Result<PathBuf> {
    ensure_with(|k| std::env::var_os(k)).await
}

async fn ensure_with(env_get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<PathBuf> {
    if let Some(override_path) = env_get("LNS_KERNEL_PATH") {
        let p = PathBuf::from(override_path);
        if !p.is_file() {
            bail!(
                "LNS_KERNEL_PATH={} is not a regular file. Set the env var to \
                 a host-readable uncompressed Linux kernel `Image`, or unset \
                 it to fall back to the CDN-published Kata {KERNEL_VERSION}.",
                p.display()
            );
        }
        let display_path = p.display();
        log::debug!("using kernel from LNS_KERNEL_PATH override: {display_path}");
        return Ok(p);
    }

    let cache = crate::cache::root()?.join("kernel");
    let cdn_base = env_get("LNS_KERNEL_CDN")
        .and_then(|v| v.into_string().ok())
        .unwrap_or_else(|| CDN_BASE.to_string());
    ensure_inner(
        &RealFetcher,
        &RealFs,
        &cache,
        &cdn_base,
        KERNEL_VERSION,
        ARCH,
        KERNEL_SHA256,
    )
    .await
}

#[allow(clippy::cognitive_complexity)] // cache check → sha verify → conditional download → atomic install
async fn ensure_inner(
    fetcher: &impl Fetcher,
    fs: &impl Fs,
    cache: &Path,
    cdn_base: &str,
    version: &str,
    arch: &str,
    expected_sha256: &str,
) -> Result<PathBuf> {
    fs.create_dir_all(cache)
        .await
        .with_context(|| format!("create_dir_all {}", cache.display()))?;
    let filename = format!("Image-{version}-{arch}");
    let path = cache.join(&filename);

    if fs.is_file(&path).await {
        match fs.read(&path).await {
            Ok(bytes) => {
                let actual = format!("{:x}", Sha256::digest(&bytes));
                if actual == expected_sha256 {
                    return Ok(path);
                }
                let path_str = path.display();
                log::warn!(
                    "cached kernel at {path_str} has wrong sha256 ({actual} vs expected {expected_sha256}) — re-downloading"
                );
            }
            Err(e) => {
                let path_str = path.display();
                let cause = format!("{e:#}");
                log::warn!("cached kernel at {path_str} is unreadable ({cause}) — re-downloading");
            }
        }
    }

    let url = format!("{cdn_base}/lns-kernel-{version}-{arch}");
    log::info!("Fetching", "guest kernel v{version} (~17 MiB, one-time)");
    log::debug!(url = %url, "downloading kernel");
    let bytes = fetcher.fetch(&url).await?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected_sha256 {
        bail!(
            "kernel sha256 mismatch — expected {expected_sha256}, got {actual} \
             for {url}. Either the published artifact was rotated without a \
             pin bump (check kernel.rs::KERNEL_SHA256), or the download was \
             tampered. Refusing to install."
        );
    }
    atomic_write(fs, &path, &bytes)
        .await
        .with_context(|| format!("installing {}", path.display()))?;
    Ok(path)
}

async fn atomic_write(fs: &impl Fs, path: &Path, bytes: &[u8]) -> Result<()> {
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
    fs.rename(&tmp, path)
        .await
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn kernel_version_has_expected_shape() {
        let parts: Vec<&str> = KERNEL_VERSION.splitn(2, '-').collect();
        let parts_msg = format!("KERNEL_VERSION={KERNEL_VERSION:?} must be <semver>-<buildnum>");
        assert_eq!(parts.len(), 2, "{parts_msg}");
        let semver_str = parts[0];
        let build_str = parts[1];
        let semver_parts: Vec<&str> = semver_str.split('.').collect();
        let semver_msg =
            format!("KERNEL_VERSION semver portion {semver_str:?} must be major.minor.patch");
        assert_eq!(semver_parts.len(), 3, "{semver_msg}");
        let all_digits = semver_parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        let parts_msg =
            format!("KERNEL_VERSION semver parts {semver_parts:?} must all be non-empty integers");
        assert!(all_digits, "{parts_msg}");
        let build_cond = !build_str.is_empty() && build_str.chars().all(|c| c.is_ascii_digit());
        let build_msg =
            format!("KERNEL_VERSION build part {build_str:?} must be a non-empty integer");
        assert!(build_cond, "{build_msg}");
    }

    #[test]
    fn pin_is_64_hex_chars() {
        assert_eq!(KERNEL_SHA256.len(), 64);
        assert!(KERNEL_SHA256.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn cdn_url_has_expected_shape() {
        let url = format!("{CDN_BASE}/lns-kernel-{KERNEL_VERSION}-{ARCH}");
        assert!(
            url.starts_with("https://get.lns.run/lns-kernel-"),
            "url {url:?} should start with the CDN base + artifact prefix"
        );
        assert!(
            url.ends_with(&format!("-{ARCH}")),
            "url {url:?} should end with -{ARCH}"
        );
        assert!(
            url.contains(KERNEL_VERSION),
            "url {url:?} should embed KERNEL_VERSION={KERNEL_VERSION}"
        );
    }

    #[tokio::test]
    async fn lns_kernel_path_env_var_overrides() {
        init_tracing_capture();
        let workspace_toml = std::env::current_dir()
            .unwrap()
            .ancestors()
            .find(|p| {
                p.join("Cargo.toml").is_file() && {
                    std::fs::read_to_string(p.join("Cargo.toml"))
                        .ok()
                        .is_some_and(|c| c.contains("[workspace]"))
                }
            })
            .map(|p| p.join("Cargo.toml"))
            .expect("workspace Cargo.toml");
        let target = workspace_toml.clone();
        let stub =
            move |k: &str| (k == "LNS_KERNEL_PATH").then(|| target.as_os_str().to_os_string());
        let resolved = ensure_with(stub).await.expect("ensure with override");
        assert_eq!(resolved, workspace_toml);
    }

    #[tokio::test]
    async fn lns_kernel_path_env_var_rejects_missing_file() {
        init_tracing_capture();
        let stub = |k: &str| {
            (k == "LNS_KERNEL_PATH").then(|| std::ffi::OsString::from("/does/not/exist/Image"))
        };
        let err = ensure_with(stub).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("LNS_KERNEL_PATH"));
        assert!(msg.contains("/does/not/exist/Image"));
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn ensure_routes_through_lns_kernel_path_override() {
        init_tracing_capture();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let _g = crate::test_env::EnvVarGuard::set("LNS_KERNEL_PATH", &path);
        let resolved = ensure().await;
        assert_eq!(resolved.expect("override should succeed"), path);
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

    const TEST_VERSION: &str = "9.9.9-test";
    const TEST_ARCH: &str = "testarch";
    const TEST_CDN: &str = "https://test.cdn.invalid";

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

    fn payload() -> Vec<u8> {
        b"test kernel bytes".to_vec()
    }

    fn payload_sha() -> String {
        format!("{:x}", Sha256::digest(payload()))
    }

    fn canonical_cache_file(cache: &std::path::Path) -> PathBuf {
        cache.join(format!("Image-{TEST_VERSION}-{TEST_ARCH}"))
    }

    #[tokio::test]
    async fn cached_kernel_with_matching_sha_returns_path_without_fetching() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        let cache_file = fake_cache_file();
        fs.put_file(&cache_file, payload());

        let fetcher = CannedFetcher::never();
        let resolved = ensure_inner(
            &fetcher,
            &fs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect("cache hit should succeed");

        assert_eq!(resolved, cache_file);
        assert!(
            fetcher.calls().is_empty(),
            "cache hit must not call the fetcher: {:?}",
            fetcher.calls()
        );
    }

    #[tokio::test]
    async fn cached_kernel_with_wrong_sha_redownloads_via_fetcher() {
        init_tracing_capture();
        let d = tempfile::TempDir::new().unwrap();
        let cache = d.path().join("kernel");
        std::fs::create_dir_all(&cache).unwrap();
        let cache_file = canonical_cache_file(&cache);
        std::fs::write(&cache_file, b"corrupted bytes").unwrap();

        let fetcher = CannedFetcher::ok_once(payload());
        let resolved = ensure_inner(
            &fetcher,
            &RealFs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect("wrong-sha cache should self-heal");

        assert_eq!(resolved, cache_file);
        let calls = fetcher.calls();
        assert_eq!(
            calls.len(),
            1,
            "wrong-sha cache must fetch exactly once: {calls:?}"
        );
        let on_disk = std::fs::read(&cache_file).unwrap();
        assert_eq!(on_disk, payload(), "cache must hold refreshed bytes");
    }

    #[tokio::test]
    async fn fetched_bytes_with_correct_sha_land_at_canonical_path() {
        init_tracing_capture();
        let d = tempfile::TempDir::new().unwrap();
        let cache = d.path().join("kernel");

        let fetcher = CannedFetcher::ok_once(payload());
        let resolved = ensure_inner(
            &fetcher,
            &RealFs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect("happy path");

        let cache_file = canonical_cache_file(&cache);
        assert_eq!(resolved, cache_file);
        assert!(cache_file.is_file(), "kernel must land at canonical path");
        let on_disk = std::fs::read(&cache_file).unwrap();
        assert_eq!(on_disk, payload());

        let tmp = cache_file.with_extension("tmp");
        assert!(!tmp.exists(), "tmp file must not remain after success");
    }

    #[tokio::test]
    async fn fetched_bytes_with_wrong_sha_bail_before_atomic_write() {
        init_tracing_capture();
        let d = tempfile::TempDir::new().unwrap();
        let cache = d.path().join("kernel");

        let fetcher = CannedFetcher::ok_once(b"tampered bytes".to_vec());
        let err = ensure_inner(
            &fetcher,
            &RealFs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect_err("sha mismatch must bail");

        let msg = format!("{err:#}");
        let has_category = msg.contains("sha256 mismatch");
        let has_verb = msg.contains("Refusing to install");
        assert!(has_category, "category in message: {msg}");
        assert!(has_verb, "actionable verb in message: {msg}");

        let cache_file = canonical_cache_file(&cache);
        let landed = cache_file.exists();
        assert!(!landed, "tampered kernel must not land on disk");
        let tmp = cache_file.with_extension("tmp");
        assert!(!tmp.exists(), "tmp file must not remain after sha mismatch");
    }

    #[tokio::test]
    async fn fetcher_error_propagates_from_ensure_inner() {
        init_tracing_capture();
        let d = tempfile::TempDir::new().unwrap();
        let cache = d.path().join("kernel");

        let fetcher = CannedFetcher::err_once(anyhow::anyhow!("simulated transport error"));
        let err = ensure_inner(
            &fetcher,
            &RealFs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect_err("fetcher failure must propagate");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("simulated transport error"),
            "underlying fetcher error must surface: {msg}"
        );

        let cache_file = canonical_cache_file(&cache);
        assert!(
            !cache_file.exists(),
            "failed fetch must not leave a file behind"
        );
    }

    #[tokio::test]
    async fn cache_dir_create_failure_is_surfaced_with_context() {
        init_tracing_capture();
        let d = tempfile::TempDir::new().unwrap();
        let cache = d.path().join("kernel-as-file");
        std::fs::write(&cache, b"i am a file, not a dir").unwrap();

        let fetcher = CannedFetcher::never();
        let err = ensure_inner(
            &fetcher,
            &RealFs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect_err("create_dir_all on a regular file must fail");

        let msg = format!("{err:#}");
        assert!(msg.contains("create_dir_all"), "context in message: {msg}");
        assert!(
            msg.contains(cache.to_str().unwrap()),
            "cache path in message: {msg}"
        );
        assert!(
            fetcher.calls().is_empty(),
            "mkdir failure must bail before fetching"
        );
    }

    #[derive(Default)]
    struct FakeState {
        files: std::collections::HashMap<PathBuf, Vec<u8>>,
        fail_read: Option<io::Error>,
        fail_remove_file: Option<io::Error>,
        fail_create_new: Option<io::Error>,
        fail_write_all: Option<io::Error>,
        fail_sync_all: Option<io::Error>,
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
        fn fail_next_rename(&self, e: io::Error) {
            self.state.lock().unwrap().fail_rename = Some(e);
        }
    }

    impl Fs for FakeFs {
        type WritableFile = FakeWritableFile;

        async fn create_dir_all(&self, _p: &Path) -> io::Result<()> {
            Ok(())
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
            let bytes =
                entry.unwrap_or_else(|| panic!("FakeFs::read: no fixture for {}", p.display()));
            Ok(bytes)
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

    fn fake_cache_root() -> PathBuf {
        PathBuf::from("/fake/cache/kernel")
    }

    fn fake_cache_file() -> PathBuf {
        fake_cache_root().join(format!("Image-{TEST_VERSION}-{TEST_ARCH}"))
    }

    #[tokio::test]
    async fn cached_kernel_unreadable_falls_through_to_fetcher() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        let cache_file = fake_cache_file();
        fs.put_file(&cache_file, b"unreadable junk");
        fs.fail_next_read(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));

        let fetcher = CannedFetcher::ok_once(payload());
        let resolved = ensure_inner(
            &fetcher,
            &fs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect("unreadable cache must self-heal via fetcher");

        assert_eq!(resolved, cache_file);
        assert_eq!(
            fetcher.calls().len(),
            1,
            "unreadable cache must trigger exactly one fetch"
        );
        assert_eq!(fs.read_file(&cache_file), Some(payload()));
    }

    #[tokio::test]
    async fn atomic_install_pre_existing_tmp_remove_failure_is_silent() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        fs.fail_next_remove_file(io::Error::new(io::ErrorKind::PermissionDenied, "stale"));

        let fetcher = CannedFetcher::ok_once(payload());
        let resolved = ensure_inner(
            &fetcher,
            &fs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect("remove_file failure must be discarded silently");

        let cache_file = fake_cache_file();
        assert_eq!(resolved, cache_file);
        assert_eq!(fs.read_file(&cache_file), Some(payload()));
    }

    #[tokio::test]
    async fn atomic_install_create_new_failure_surfaces_with_path() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        fs.fail_next_create_new(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));

        let fetcher = CannedFetcher::ok_once(payload());
        let err = ensure_inner(
            &fetcher,
            &fs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect_err("create_new failure must propagate");

        let msg = format!("{err:#}");
        let cache_file = fake_cache_file();
        let tmp = cache_file.with_extension("tmp");
        assert!(msg.contains("installing"), "outer context: {msg}");
        assert!(
            msg.contains(cache_file.to_str().unwrap()),
            "destination path: {msg}"
        );
        assert!(msg.contains("creating"), "inner context: {msg}");
        assert!(msg.contains(tmp.to_str().unwrap()), "tmp path: {msg}");
        assert!(!fs.file_exists(&cache_file), "no file at destination");
    }

    #[tokio::test]
    async fn atomic_install_write_all_failure_surfaces_with_tmp_path() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        fs.fail_next_write_all(io::Error::other("disk full"));

        let fetcher = CannedFetcher::ok_once(payload());
        let err = ensure_inner(
            &fetcher,
            &fs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect_err("write_all failure must propagate");

        let msg = format!("{err:#}");
        let cache_file = fake_cache_file();
        let tmp = cache_file.with_extension("tmp");
        assert!(msg.contains("writing"), "inner context: {msg}");
        assert!(msg.contains(tmp.to_str().unwrap()), "tmp path: {msg}");
        assert!(!fs.file_exists(&cache_file), "no file at destination");
    }

    #[tokio::test]
    async fn atomic_install_sync_all_failure_surfaces_with_tmp_path() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        fs.fail_next_sync_all(io::Error::other("fsync failed"));

        let fetcher = CannedFetcher::ok_once(payload());
        let err = ensure_inner(
            &fetcher,
            &fs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect_err("sync_all failure must propagate");

        let msg = format!("{err:#}");
        let cache_file = fake_cache_file();
        let tmp = cache_file.with_extension("tmp");
        assert!(msg.contains("fsync"), "fsync context: {msg}");
        assert!(msg.contains(tmp.to_str().unwrap()), "tmp path: {msg}");
        assert!(!fs.file_exists(&cache_file), "no file at destination");
    }

    #[tokio::test]
    async fn atomic_install_rename_failure_surfaces_with_both_paths() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        fs.fail_next_rename(io::Error::other("rename failed"));

        let fetcher = CannedFetcher::ok_once(payload());
        let err = ensure_inner(
            &fetcher,
            &fs,
            &cache,
            TEST_CDN,
            TEST_VERSION,
            TEST_ARCH,
            &payload_sha(),
        )
        .await
        .expect_err("rename failure must propagate");

        let msg = format!("{err:#}");
        let cache_file = fake_cache_file();
        let tmp = cache_file.with_extension("tmp");
        assert!(msg.contains("rename"), "context: {msg}");
        assert!(msg.contains(tmp.to_str().unwrap()), "tmp in msg: {msg}");
        assert!(
            msg.contains(cache_file.to_str().unwrap()),
            "destination in msg: {msg}"
        );
        assert!(!fs.file_exists(&cache_file), "no file at destination");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn ensure_production_wiring_uses_lns_kernel_cdn_and_rejects_sha_mismatch() {
        init_tracing_capture();
        let server = wiremock::MockServer::start().await;
        let expected_path = format!("/lns-kernel-{KERNEL_VERSION}-{ARCH}");
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(expected_path.clone()))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_bytes(b"not the real kernel".as_slice()),
            )
            .expect(1)
            .mount(&server)
            .await;

        let cache_root = tempfile::TempDir::new().unwrap();
        let _path = crate::test_env::EnvVarGuard::unset("LNS_KERNEL_PATH");
        let _cdn = crate::test_env::EnvVarGuard::set("LNS_KERNEL_CDN", server.uri());
        let _home = crate::test_env::EnvVarGuard::set("HOME", cache_root.path());
        let _xdg =
            crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", cache_root.path().join("xdg"));
        let result = ensure().await;

        let err = result.expect_err("wiremock bytes won't match KERNEL_SHA256");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("sha256 mismatch"),
            "sha mismatch surface: {msg}"
        );
        assert!(
            msg.contains(&format!("{}{}", server.uri(), expected_path)),
            "URL must appear in error message: {msg}"
        );
        assert!(
            msg.contains("Refusing to install"),
            "actionable verb: {msg}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn real_fetcher_wraps_http_error_with_url_context() {
        init_tracing_capture();
        let server = wiremock::MockServer::start().await;
        let expected_path = format!("/lns-kernel-{KERNEL_VERSION}-{ARCH}");
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(expected_path.clone()))
            .respond_with(wiremock::ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let cache_root = tempfile::TempDir::new().unwrap();
        let _path = crate::test_env::EnvVarGuard::unset("LNS_KERNEL_PATH");
        let _cdn = crate::test_env::EnvVarGuard::set("LNS_KERNEL_CDN", server.uri());
        let _home = crate::test_env::EnvVarGuard::set("HOME", cache_root.path());
        let _xdg =
            crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", cache_root.path().join("xdg"));
        let result = ensure().await;

        let err = result.expect_err("503 must bail");
        let msg = format!("{err:#}");
        assert!(msg.contains("HTTP error from"), "context: {msg}");
        assert!(
            msg.contains(&format!("{}{}", server.uri(), expected_path)),
            "URL in error: {msg}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn real_fetcher_wraps_transport_error_with_url_context() {
        init_tracing_capture();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let cache_root = tempfile::TempDir::new().unwrap();
        let _path = crate::test_env::EnvVarGuard::unset("LNS_KERNEL_PATH");
        let _cdn = crate::test_env::EnvVarGuard::set("LNS_KERNEL_CDN", format!("http://{addr}"));
        let _home = crate::test_env::EnvVarGuard::set("HOME", cache_root.path());
        let _xdg =
            crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", cache_root.path().join("xdg"));
        let result = ensure().await;

        let err = result.expect_err("connect-refused must bail");
        let msg = format!("{err:#}");
        assert!(msg.contains("downloading"), "context: {msg}");
        assert!(
            msg.contains(&format!("http://{addr}")),
            "URL in error: {msg}"
        );
    }
}
