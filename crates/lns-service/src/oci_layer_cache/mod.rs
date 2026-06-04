mod install;
#[cfg(test)]
mod testfs;

use crate::log;
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub struct LayerCache {
    root: PathBuf,
    fs: Arc<dyn install::Fs>,
}

impl std::fmt::Debug for LayerCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayerCache")
            .field("root", &self.root)
            .finish()
    }
}

impl LayerCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_fs(root, Arc::new(install::RealFs))
    }

    fn with_fs(root: impl Into<PathBuf>, fs: Arc<dyn install::Fs>) -> Self {
        Self {
            root: root.into(),
            fs,
        }
    }

    pub fn path_for(&self, digest: &str) -> Result<PathBuf> {
        let hex = parse_sha256(digest)?;
        Ok(self.root.join("sha256").join(hex))
    }

    pub fn contains(&self, digest: &str) -> Result<bool> {
        Ok(self.fs.is_file(&self.path_for(digest)?))
    }

    pub fn install_from_bytes(&self, expected_digest: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.path_for(expected_digest)?;
        if self.fs.is_file(&path) {
            return Ok(path);
        }
        install::install_from_bytes(self.fs.as_ref(), &path, expected_digest, bytes)?;
        Ok(path)
    }

    // re-verify sha256 on read — a mismatch is corruption or tampering, not a refetch
    pub fn read(&self, expected_digest: &str) -> Result<Vec<u8>> {
        let path = self.path_for(expected_digest)?;
        let bytes = self
            .fs
            .read(&path)
            .with_context(|| format!("reading cached layer {}", path.display()))?;
        use sha2::{Digest, Sha256};
        let actual = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
        if actual != expected_digest {
            anyhow::bail!(
                "cached layer at {} has wrong sha256 ({}); expected {}",
                path.display(),
                actual,
                expected_digest
            );
        }
        Ok(bytes)
    }

    pub async fn get_or_install<F, Fut>(&self, digest: &str, fetcher: F) -> Result<Vec<u8>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>>>,
    {
        let path = self.path_for(digest)?;
        if self.fs.is_file(&path) {
            match self.read(digest) {
                Ok(bytes) => return Ok(bytes),
                Err(e) => {
                    log::warn!("cache entry for {digest} unreadable ({e:#}); will refetch");
                }
            }
        }
        let bytes = fetcher()
            .await
            .with_context(|| format!("fetching {digest}"))?;
        install::install_from_bytes(self.fs.as_ref(), &path, digest, &bytes)?;
        Ok(bytes)
    }

    pub fn install_from_reader<R: std::io::Read>(
        &self,
        expected_digest: &str,
        reader: R,
    ) -> Result<PathBuf> {
        let path = self.path_for(expected_digest)?;
        if self.fs.is_file(&path) {
            return Ok(path);
        }
        install::install_from_reader(self.fs.as_ref(), &path, expected_digest, reader)?;
        Ok(path)
    }

    pub fn sweep(&self, keep: &HashSet<String>) -> Result<u64> {
        let dir = self.root.join("sha256");
        if !self.fs.is_dir(&dir) {
            return Ok(0);
        }
        let keep_hex: HashSet<String> = keep
            .iter()
            .map(|d| d.strip_prefix("sha256:").unwrap_or(d).to_ascii_lowercase())
            .collect();

        let entries = self
            .fs
            .read_dir_entries(&dir)
            .with_context(|| format!("read_dir {}", dir.display()))?;

        let mut freed = 0u64;
        for entry in entries {
            let name_str = match entry.name.to_str() {
                Some(s) => s,
                None => continue,
            };
            // Don't touch *.tmp files — a concurrent install owns them; a crashed install's tmp gets cleaned on the next install.
            if name_str.ends_with(".tmp") {
                continue;
            }
            if keep_hex.contains(name_str) {
                continue;
            }
            self.fs
                .remove_file(&entry.path)
                .with_context(|| format!("sweep: removing {}", entry.path.display()))?;
            freed += entry.size;
        }
        Ok(freed)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn parse_sha256(digest: &str) -> Result<String> {
    let (algo, hex) = digest
        .split_once(':')
        .with_context(|| format!("digest {digest:?} missing `<algo>:<hex>` colon"))?;
    if algo != "sha256" {
        bail!("unsupported digest algorithm: {algo:?} (expected sha256)");
    }
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("digest hex must be 64 lowercase hex chars, got {hex:?}");
    }
    Ok(hex.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci_layer_cache::testfs::InMemoryFs;
    use sha2::{Digest, Sha256};
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicBool, Ordering};

    const ROOT: &str = "/cache";

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn fixture() -> (LayerCache, InMemoryFs) {
        let fs = InMemoryFs::new();
        let cache = LayerCache::with_fs(ROOT, fs.as_arc());
        (cache, fs)
    }

    #[test]
    fn parse_sha256_accepts_canonical_form() {
        let hex = "abcd".repeat(16);
        assert_eq!(parse_sha256(&format!("sha256:{hex}")).unwrap(), hex);
    }

    #[test]
    fn parse_sha256_rejects_missing_colon() {
        let err = parse_sha256("sha256-abc").unwrap_err();
        assert!(format!("{err:#}").contains("colon"));
    }

    #[test]
    fn parse_sha256_rejects_other_algorithms() {
        let err = parse_sha256(&format!("sha512:{}", "0".repeat(64))).unwrap_err();
        assert!(format!("{err:#}").contains("sha256"));
    }

    #[test]
    fn parse_sha256_rejects_wrong_length() {
        let err = parse_sha256("sha256:abc").unwrap_err();
        assert!(format!("{err:#}").contains("64"));
    }

    #[test]
    fn parse_sha256_rejects_non_hex() {
        let err = parse_sha256(&format!("sha256:{}", "z".repeat(64))).unwrap_err();
        assert!(format!("{err:#}").contains("hex"));
    }

    #[test]
    fn path_for_uses_sha256_subdir() {
        let (cache, _) = fixture();
        let hex = "ab".repeat(32);
        let p = cache.path_for(&format!("sha256:{hex}")).unwrap();
        assert_eq!(p, PathBuf::from(ROOT).join("sha256").join(&hex));
    }

    #[test]
    fn path_for_propagates_parse_failure() {
        let (cache, _) = fixture();
        let err = cache.path_for("not-a-digest").unwrap_err();
        assert!(format!("{err:#}").contains("colon"));
    }

    #[test]
    fn contains_missing_blob_is_false() {
        let (cache, _) = fixture();
        assert!(
            !cache
                .contains(&format!("sha256:{}", "a".repeat(64)))
                .unwrap()
        );
    }

    #[test]
    fn root_returns_underlying_directory() {
        let (cache, _) = fixture();
        assert_eq!(cache.root(), Path::new(ROOT));
    }

    #[test]
    fn new_wires_real_fs_for_production_use() {
        let cache = LayerCache::new("/some/dir");
        assert_eq!(cache.root(), Path::new("/some/dir"));
    }

    #[test]
    fn debug_impl_includes_root_path() {
        let (cache, _) = fixture();
        let rendered = format!("{cache:?}");
        assert!(rendered.contains("LayerCache"));
        assert!(rendered.contains(ROOT));
    }

    #[test]
    fn install_writes_bytes_and_verifies_digest() {
        let (cache, fs) = fixture();
        let bytes = b"hello world";
        let digest = format!("sha256:{}", sha256_hex(bytes));
        let path = cache.install_from_bytes(&digest, bytes).unwrap();
        assert!(cache.contains(&digest).unwrap());
        assert_eq!(fs.read_bytes(&path).unwrap(), bytes);
    }

    #[test]
    fn install_rejects_wrong_digest() {
        let (cache, _) = fixture();
        let wrong = format!("sha256:{}", "0".repeat(64));
        let err = cache
            .install_from_bytes(&wrong, b"hello world")
            .unwrap_err();
        assert!(format!("{err:#}").contains("digest mismatch"));
    }

    #[test]
    fn install_is_idempotent_via_short_circuit() {
        let (cache, fs) = fixture();
        let bytes = b"hello";
        let digest = format!("sha256:{}", sha256_hex(bytes));
        let a = cache.install_from_bytes(&digest, bytes).unwrap();
        let b = cache.install_from_bytes(&digest, bytes).unwrap();
        assert_eq!(a, b);
        assert_eq!(fs.sync_calls().len(), 1);
    }

    #[test]
    fn install_from_reader_round_trips_through_cache() {
        let (cache, fs) = fixture();
        let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let digest = format!("sha256:{}", sha256_hex(&bytes));
        let path = cache.install_from_reader(&digest, &bytes[..]).unwrap();
        assert_eq!(fs.read_bytes(&path).unwrap(), bytes);
    }

    #[test]
    fn install_from_reader_short_circuits_on_cache_hit() {
        let (cache, fs) = fixture();
        let bytes = b"cached";
        let digest = format!("sha256:{}", sha256_hex(bytes));
        cache.install_from_bytes(&digest, bytes).unwrap();
        let syncs_before = fs.sync_calls().len();
        cache
            .install_from_reader(&digest, std::io::empty())
            .unwrap();
        assert_eq!(fs.sync_calls().len(), syncs_before);
    }

    #[test]
    fn read_returns_bytes_when_digest_matches() {
        let (cache, _) = fixture();
        let bytes = b"hello cached layer";
        let digest = format!("sha256:{}", sha256_hex(bytes));
        cache.install_from_bytes(&digest, bytes).unwrap();
        assert_eq!(cache.read(&digest).unwrap(), bytes);
    }

    #[test]
    fn read_errors_when_blob_missing() {
        let (cache, _) = fixture();
        let absent = format!("sha256:{}", "b".repeat(64));
        let err = cache.read(&absent).unwrap_err();
        assert!(format!("{err:#}").contains("reading cached layer"));
    }

    #[test]
    fn read_detects_on_disk_tamper() {
        let (cache, fs) = fixture();
        let bytes = b"original";
        let digest = format!("sha256:{}", sha256_hex(bytes));
        let path = cache.install_from_bytes(&digest, bytes).unwrap();
        fs.put(&path, b"tampered");
        let err = cache.read(&digest).unwrap_err();
        assert!(format!("{err:#}").contains("wrong sha256"));
    }

    #[tokio::test]
    async fn get_or_install_returns_cached_without_calling_fetcher() {
        let (cache, fs) = fixture();
        let bytes = b"already cached";
        let digest = format!("sha256:{}", sha256_hex(bytes));
        cache.install_from_bytes(&digest, bytes).unwrap();
        let syncs_before = fs.sync_calls().len();
        let got = cache
            .get_or_install(&digest, || async { Ok(vec![0u8; 8]) })
            .await
            .unwrap();
        assert_eq!(got, bytes);
        assert_eq!(fs.sync_calls().len(), syncs_before);
    }

    #[tokio::test]
    async fn get_or_install_calls_fetcher_on_miss() {
        let (cache, _) = fixture();
        let bytes = b"fetched lazily";
        let digest = format!("sha256:{}", sha256_hex(bytes));
        let got = cache
            .get_or_install(&digest, || async move { Ok(bytes.to_vec()) })
            .await
            .unwrap();
        assert_eq!(got, bytes);
        assert!(cache.contains(&digest).unwrap());
    }

    #[tokio::test]
    async fn get_or_install_propagates_fetcher_error() {
        let (cache, _) = fixture();
        let digest = format!("sha256:{}", "a".repeat(64));
        let err = cache
            .get_or_install(&digest, || async { anyhow::bail!("registry returned 502") })
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("registry returned 502"));
        assert!(!cache.contains(&digest).unwrap());
    }

    #[tokio::test]
    async fn get_or_install_self_heals_corrupted_cache_entry() {
        static SUB_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        SUB_INIT.get_or_init(|| {
            let sub = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_writer(std::io::sink)
                .finish();
            let _ = tracing::subscriber::set_global_default(sub);
        });

        let (cache, fs) = fixture();
        let original = b"original";
        let digest = format!("sha256:{}", sha256_hex(original));
        let path = cache.install_from_bytes(&digest, original).unwrap();
        fs.put(&path, b"tampered with");

        let fetcher_called = std::sync::Arc::new(AtomicBool::new(false));
        let fetcher_called_clone = fetcher_called.clone();
        let got = cache
            .get_or_install(&digest, move || {
                fetcher_called_clone.store(true, Ordering::SeqCst);
                async move { Ok(original.to_vec()) }
            })
            .await
            .unwrap();
        assert_eq!(got, original);
        assert!(fetcher_called.load(Ordering::SeqCst));
        assert_eq!(cache.read(&digest).unwrap(), original);
    }

    #[tokio::test]
    async fn get_or_install_propagates_install_failure_for_bad_fetcher_bytes() {
        let (cache, _) = fixture();
        let wrong = format!("sha256:{}", "f".repeat(64));
        let err = cache
            .get_or_install(&wrong, || async { Ok(b"unrelated content".to_vec()) })
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("digest mismatch"));
        assert!(!cache.contains(&wrong).unwrap());
    }

    #[test]
    fn sweep_removes_unkept_blobs() {
        let (cache, _) = fixture();
        let keep_bytes = b"keep me";
        let drop_bytes = b"drop me";
        let keep_digest = format!("sha256:{}", sha256_hex(keep_bytes));
        let drop_digest = format!("sha256:{}", sha256_hex(drop_bytes));
        cache.install_from_bytes(&keep_digest, keep_bytes).unwrap();
        cache.install_from_bytes(&drop_digest, drop_bytes).unwrap();
        let mut keep = HashSet::new();
        keep.insert(keep_digest.clone());
        let freed = cache.sweep(&keep).unwrap();
        assert_eq!(freed, drop_bytes.len() as u64);
        assert!(cache.contains(&keep_digest).unwrap());
        assert!(!cache.contains(&drop_digest).unwrap());
    }

    #[test]
    fn sweep_strips_sha256_prefix_in_keep_set() {
        let (cache, _) = fixture();
        let bytes = b"keep";
        let digest = format!("sha256:{}", sha256_hex(bytes));
        cache.install_from_bytes(&digest, bytes).unwrap();
        let mut keep = HashSet::new();
        keep.insert(sha256_hex(bytes));
        let freed = cache.sweep(&keep).unwrap();
        assert_eq!(freed, 0);
        assert!(cache.contains(&digest).unwrap());
    }

    #[test]
    fn sweep_accepts_uppercase_hex_in_keep_set() {
        let (cache, _) = fixture();
        let bytes = b"keep";
        let lowercase_hex = sha256_hex(bytes);
        cache
            .install_from_bytes(&format!("sha256:{lowercase_hex}"), bytes)
            .unwrap();
        let mut keep = HashSet::new();
        keep.insert(format!("sha256:{}", lowercase_hex.to_ascii_uppercase()));
        let freed = cache.sweep(&keep).unwrap();
        assert_eq!(freed, 0);
        assert!(cache.contains(&format!("sha256:{lowercase_hex}")).unwrap());
    }

    #[test]
    fn sweep_ignores_tmp_files() {
        let (cache, fs) = fixture();
        let dir = PathBuf::from(ROOT).join("sha256");
        fs.put(&dir.join("stray.tmp"), b"in-flight");
        let freed = cache.sweep(&HashSet::new()).unwrap();
        assert_eq!(freed, 0);
        assert!(fs.contains(&dir.join("stray.tmp")));
    }

    #[test]
    fn sweep_skips_non_utf8_filenames() {
        let (cache, fs) = fixture();
        let dir = PathBuf::from(ROOT).join("sha256");
        let keep_bytes = b"keep";
        let keep_digest = format!("sha256:{}", sha256_hex(keep_bytes));
        cache.install_from_bytes(&keep_digest, keep_bytes).unwrap();
        #[cfg(unix)]
        let bad_name = {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![0xff, 0xfe])
        };
        #[cfg(not(unix))]
        let bad_name = OsString::from("non_utf8_placeholder");
        fs.put_dir_entry(&dir, bad_name, 123);
        let mut keep = HashSet::new();
        keep.insert(keep_digest.clone());
        let freed = cache.sweep(&keep).unwrap();
        assert_eq!(freed, 0);
    }

    #[test]
    fn sweep_on_missing_sha256_dir_is_no_op() {
        let (cache, _) = fixture();
        let freed = cache.sweep(&HashSet::new()).unwrap();
        assert_eq!(freed, 0);
    }

    #[test]
    fn sweep_propagates_read_dir_failure() {
        let (cache, fs) = fixture();
        let bytes = b"x";
        let digest = format!("sha256:{}", sha256_hex(bytes));
        cache.install_from_bytes(&digest, bytes).unwrap();
        fs.inject(|f| f.fail_read_dir = Some(std::io::ErrorKind::PermissionDenied));
        let err = cache.sweep(&HashSet::new()).unwrap_err();
        assert!(format!("{err:#}").contains("read_dir"));
    }

    #[test]
    fn sweep_propagates_remove_file_failure() {
        let (cache, fs) = fixture();
        let bytes = b"drop me";
        let digest = format!("sha256:{}", sha256_hex(bytes));
        cache.install_from_bytes(&digest, bytes).unwrap();
        fs.inject(|f| f.fail_remove_file = Some(std::io::ErrorKind::PermissionDenied));
        let err = cache.sweep(&HashSet::new()).unwrap_err();
        assert!(format!("{err:#}").contains("sweep: removing"));
    }

    #[tokio::test]
    async fn read_propagates_fs_read_failure() {
        let (cache, fs) = fixture();
        let bytes = b"hi";
        let digest = format!("sha256:{}", sha256_hex(bytes));
        cache.install_from_bytes(&digest, bytes).unwrap();
        fs.inject(|f| f.fail_read = Some(std::io::ErrorKind::PermissionDenied));
        let err = cache.read(&digest).unwrap_err();
        assert!(format!("{err:#}").contains("reading cached layer"));
    }
}
