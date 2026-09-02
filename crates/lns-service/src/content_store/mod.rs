#![allow(dead_code)]

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct ContentStore {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallResult {
    pub digest: String,
    pub raw_digest: [u8; 32],
    pub path: PathBuf,
    pub size: u64,
}

impl ContentStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path_for(&self, digest: &str) -> Result<PathBuf> {
        let hex = parse_sha256(digest)?;
        Ok(self.root.join("sha256").join(hex))
    }

    pub fn contains(&self, digest: &str) -> Result<bool> {
        Ok(self.path_for(digest)?.is_file())
    }

    pub(crate) fn staging_path(&self) -> Result<PathBuf> {
        let parent = self.root.join("sha256");
        std::fs::create_dir_all(&parent)
            .with_context(|| format!("create_dir_all {}", parent.display()))?;
        Ok(unique_tmp_in(&parent))
    }

    pub(crate) fn commit_verified(
        &self,
        staged: PathBuf,
        expected_digest: &str,
        expected_size: u64,
    ) -> Result<InstallResult> {
        let outcome = (|| -> Result<InstallResult> {
            let expected_hex = parse_sha256(expected_digest)?;
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&staged)
                .with_context(|| format!("opening staged content {}", staged.display()))?;
            let mut hasher = Sha256::new();
            let mut size = 0u64;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = std::io::Read::read(&mut file, &mut buf)
                    .with_context(|| format!("reading staged content {}", staged.display()))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                size = size.saturating_add(n as u64);
            }
            if size != expected_size {
                bail!("content size mismatch — expected {expected_size} bytes, received {size}");
            }
            let raw: [u8; 32] = hasher.finalize().into();
            let actual_hex = hex::encode(raw);
            if actual_hex != expected_hex {
                bail!(
                    "content digest mismatch — expected sha256:{expected_hex}, got sha256:{actual_hex}"
                );
            }
            file.sync_all().context("fsync staged content blob")?;
            drop(file);
            let final_path = self.root.join("sha256").join(&expected_hex);
            if final_path.is_file() {
                std::fs::remove_file(&staged)
                    .with_context(|| format!("removing staged content {}", staged.display()))?;
            } else {
                std::fs::rename(&staged, &final_path).with_context(|| {
                    format!("rename {} -> {}", staged.display(), final_path.display())
                })?;
            }
            Ok(InstallResult {
                digest: format!("sha256:{expected_hex}"),
                raw_digest: raw,
                path: final_path,
                size,
            })
        })();
        if outcome.is_err() {
            let _ = std::fs::remove_file(staged);
        }
        outcome
    }

    pub fn install_from_reader<R: std::io::Read>(&self, mut reader: R) -> Result<InstallResult> {
        let parent = self.root.join("sha256");
        std::fs::create_dir_all(&parent)
            .with_context(|| format!("create_dir_all {}", parent.display()))?;
        let tmp_name = unique_tmp_in(&parent);
        let mut hasher = Sha256::new();
        let mut size = 0u64;

        let write_outcome = (|| -> Result<()> {
            let mut f = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&tmp_name)
                .with_context(|| format!("creating {}", tmp_name.display()))?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = reader
                    .read(&mut buf)
                    .with_context(|| format!("reading into {}", tmp_name.display()))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                f.write_all(&buf[..n])
                    .with_context(|| format!("writing {n} bytes to {}", tmp_name.display()))?;
                size += n as u64;
            }
            f.sync_all().context("fsync content blob")?;
            Ok(())
        })();
        if let Err(e) = write_outcome {
            let _ = std::fs::remove_file(&tmp_name);
            return Err(e);
        }

        let raw: [u8; 32] = hasher.finalize().into();
        let digest = format!("sha256:{}", hex::encode(raw));
        let final_path = parent.join(hex::encode(raw));

        if final_path.is_file() {
            let _ = std::fs::remove_file(&tmp_name);
            return Ok(InstallResult {
                digest,
                raw_digest: raw,
                path: final_path,
                size,
            });
        }
        std::fs::rename(&tmp_name, &final_path).with_context(|| {
            format!("rename {} -> {}", tmp_name.display(), final_path.display())
        })?;
        Ok(InstallResult {
            digest,
            raw_digest: raw,
            path: final_path,
            size,
        })
    }

    pub fn install_from_bytes(&self, bytes: &[u8]) -> Result<InstallResult> {
        let raw: [u8; 32] = Sha256::digest(bytes).into();
        let digest = format!("sha256:{}", hex::encode(raw));
        let path = self.path_for(&digest)?;
        if path.is_file() {
            return Ok(InstallResult {
                digest,
                raw_digest: raw,
                path,
                size: bytes.len() as u64,
            });
        }
        atomic_write(&path, bytes)?;
        Ok(InstallResult {
            digest,
            raw_digest: raw,
            path,
            size: bytes.len() as u64,
        })
    }

    pub fn sweep(&self, keep: &HashSet<String>) -> Result<u64> {
        let dir = self.root.join("sha256");
        if !dir.is_dir() {
            return Ok(0);
        }
        let keep_hex: HashSet<&str> = keep
            .iter()
            .filter_map(|d| d.strip_prefix("sha256:").or(Some(d.as_str())))
            .collect();

        let mut freed = 0u64;
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("read_dir {}", dir.display()))?
        {
            let entry = entry?;
            let name = entry.file_name();
            if matches!(classify_sweep_entry(&name, &keep_hex), SweepAction::Remove) {
                freed += remove_blob(&entry)?;
            }
        }
        Ok(freed)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn atomic_write(final_path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = final_path
        .parent()
        .with_context(|| format!("install path {} has no parent", final_path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create_dir_all {}", parent.display()))?;
    atomic_write_via_tmp(final_path, &unique_tmp(final_path), bytes)
}

fn atomic_write_via_tmp(final_path: &Path, tmp: &Path, bytes: &[u8]) -> Result<()> {
    let write_result = (|| -> Result<()> {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("writing {} bytes to {}", bytes.len(), tmp.display()))?;
        f.sync_all().context("fsync content blob")?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(tmp);
        return Err(e);
    }

    std::fs::rename(tmp, final_path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), final_path.display()))?;
    Ok(())
}

fn tmp_token() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!(".tmp.{pid}.{nanos}.{seq}")
}

fn unique_tmp(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(tmp_token());
    path.with_file_name(name)
}

fn unique_tmp_in(dir: &Path) -> PathBuf {
    dir.join(tmp_token())
}

#[derive(Debug, PartialEq, Eq)]
enum SweepAction {
    Skip,
    Remove,
}

fn remove_blob(entry: &std::fs::DirEntry) -> Result<u64> {
    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
    std::fs::remove_file(entry.path())
        .with_context(|| format!("sweep: removing {}", entry.path().display()))?;
    Ok(size)
}

fn classify_sweep_entry(name: &std::ffi::OsStr, keep_hex: &HashSet<&str>) -> SweepAction {
    let Some(name_str) = name.to_str() else {
        return SweepAction::Skip;
    };
    if name_str.contains(".tmp.") {
        return SweepAction::Skip;
    }
    if keep_hex.contains(name_str) {
        return SweepAction::Skip;
    }
    SweepAction::Remove
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

    fn tempdir() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("tempdir")
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    struct FailingReader {
        yielded: usize,
    }

    impl std::io::Read for FailingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.yielded == 0 {
                self.yielded = buf.len().min(16);
                buf[..self.yielded].fill(0xAA);
                Ok(self.yielded)
            } else {
                Err(std::io::Error::other("simulated mid-stream failure"))
            }
        }
    }

    #[test]
    fn install_writes_bytes_and_returns_digest() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let bytes = b"hello content store";
        let r = store.install_from_bytes(bytes).unwrap();
        assert_eq!(r.digest, format!("sha256:{}", sha256_hex(bytes)));
        assert_eq!(r.size, bytes.len() as u64);
        assert!(r.path.is_file());
        assert_eq!(std::fs::read(&r.path).unwrap(), bytes);
    }

    #[test]
    fn install_from_reader_returns_same_digest_as_install_from_bytes() {
        let d = tempdir();
        let a = ContentStore::new(d.path().join("a"));
        let b = ContentStore::new(d.path().join("b"));
        let bytes = b"phase C streaming sanity";
        let bulk = a.install_from_bytes(bytes).unwrap();
        let stream = b.install_from_reader(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(bulk.digest, stream.digest);
        assert_eq!(bulk.size, stream.size);
        assert_eq!(bulk.raw_digest, stream.raw_digest);
    }

    #[test]
    fn install_from_reader_streams_large_body_correctly() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let body: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let r = store
            .install_from_reader(std::io::Cursor::new(&body[..]))
            .unwrap();
        assert_eq!(r.size, body.len() as u64);
        assert_eq!(r.digest, format!("sha256:{}", sha256_hex(&body)));
        assert_eq!(std::fs::read(&r.path).unwrap(), body);
    }

    #[test]
    fn install_from_reader_io_error_leaves_no_tmp() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let err = store
            .install_from_reader(FailingReader { yielded: 0 })
            .unwrap_err();
        assert!(format!("{err:#}").contains("simulated"));

        let dir = d.path().join("sha256");
        std::fs::write(dir.join("decoy-not-a-blob"), b"").unwrap();
        let stray: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|s| s.contains(".tmp."))
                    .unwrap_or(false)
            })
            .collect();
        assert!(stray.is_empty(), "stray tmp: {stray:?}");
    }

    #[test]
    fn commit_verified_accepts_only_the_declared_digest_and_size() {
        let d = tempfile::tempdir().unwrap();
        let store = ContentStore::new(d.path());
        let bytes = b"streamed fileset layer";
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
        let staged = store.staging_path().unwrap();
        std::fs::write(&staged, bytes).unwrap();

        let installed = store
            .commit_verified(staged, &digest, bytes.len() as u64)
            .unwrap();

        assert_eq!(installed.digest, digest);
        assert_eq!(installed.size, bytes.len() as u64);
        assert_eq!(std::fs::read(installed.path).unwrap(), bytes);
    }

    #[test]
    fn commit_verified_removes_a_partial_blob_when_declared_size_is_wrong() {
        let d = tempfile::tempdir().unwrap();
        let store = ContentStore::new(d.path());
        let bytes = b"partial";
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
        let staged = store.staging_path().unwrap();
        std::fs::write(&staged, bytes).unwrap();

        let err = store
            .commit_verified(staged.clone(), &digest, bytes.len() as u64 + 1)
            .unwrap_err();

        assert!(format!("{err:#}").contains("size mismatch"), "got: {err:#}");
        assert!(!staged.exists());
        assert!(!store.contains(&digest).unwrap());
    }

    #[test]
    fn commit_verified_removes_a_tampered_blob_before_it_becomes_addressable() {
        let d = tempfile::tempdir().unwrap();
        let store = ContentStore::new(d.path());
        let staged = store.staging_path().unwrap();
        std::fs::write(&staged, b"tampered").unwrap();
        let expected = format!("sha256:{}", "a".repeat(64));

        let err = store
            .commit_verified(staged.clone(), &expected, 8)
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("digest mismatch"),
            "got: {err:#}"
        );
        assert!(!staged.exists());
        assert!(!store.contains(&expected).unwrap());
    }

    #[test]
    fn commit_verified_removes_staging_when_the_declared_digest_is_malformed() {
        let d = tempfile::tempdir().unwrap();
        let store = ContentStore::new(d.path());
        let staged = store.staging_path().unwrap();
        std::fs::write(&staged, b"content").unwrap();

        let err = store
            .commit_verified(staged.clone(), "sha512:bad", 7)
            .unwrap_err();

        assert!(format!("{err:#}").contains("unsupported digest algorithm"));
        assert!(!staged.exists());
    }

    #[test]
    fn commit_verified_discards_a_duplicate_after_verifying_it() {
        let d = tempfile::tempdir().unwrap();
        let store = ContentStore::new(d.path());
        let bytes = b"same";
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
        for _ in 0..2 {
            let staged = store.staging_path().unwrap();
            std::fs::write(&staged, bytes).unwrap();
            store
                .commit_verified(staged, &digest, bytes.len() as u64)
                .unwrap();
        }

        assert_eq!(
            std::fs::read(store.path_for(&digest).unwrap()).unwrap(),
            bytes
        );
        assert_eq!(
            std::fs::read_dir(d.path().join("sha256")).unwrap().count(),
            1
        );
    }

    #[test]
    fn install_from_reader_empty_yields_empty_string_digest() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let r = store.install_from_reader(std::io::empty()).unwrap();
        assert_eq!(
            r.digest,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(r.size, 0);
    }

    #[test]
    fn install_is_idempotent_for_same_bytes() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let bytes = b"same bytes";
        let a = store.install_from_bytes(bytes).unwrap();
        let b = store.install_from_bytes(bytes).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn install_dedups_via_filename() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        store.install_from_bytes(b"alpha").unwrap();
        store.install_from_bytes(b"alpha").unwrap();
        store.install_from_bytes(b"beta").unwrap();
        let count = std::fs::read_dir(d.path().join("sha256")).unwrap().count();
        assert_eq!(count, 2);
    }

    #[test]
    fn install_creates_parent_directory() {
        let d = tempdir();
        let store = ContentStore::new(d.path().join("deep").join("nested"));
        let r = store.install_from_bytes(b"x").unwrap();
        assert!(r.path.is_file());
    }

    #[test]
    fn install_empty_bytes_works() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let r = store.install_from_bytes(b"").unwrap();
        assert_eq!(
            r.digest,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(r.size, 0);
    }

    #[test]
    fn install_leaves_no_tmp_on_success() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        store.install_from_bytes(b"x").unwrap();
        let entries: Vec<_> = std::fs::read_dir(d.path().join("sha256"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|s| s.contains(".tmp."))
                    .unwrap_or(false)
            })
            .collect();
        assert!(entries.is_empty(), "stray *.tmp.*: {entries:?}");
    }

    #[test]
    fn contains_after_install() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let r = store.install_from_bytes(b"x").unwrap();
        assert!(store.contains(&r.digest).unwrap());
    }

    #[test]
    fn path_for_uses_sha256_subdir() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let hex = "cd".repeat(32);
        let p = store.path_for(&format!("sha256:{hex}")).unwrap();
        assert_eq!(p, d.path().join("sha256").join(&hex));
    }

    #[test]
    fn sweep_removes_unkept_blobs() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let keep = store.install_from_bytes(b"keep").unwrap();
        let drop = store.install_from_bytes(b"drop").unwrap();
        let mut keep_set = HashSet::new();
        keep_set.insert(keep.digest.clone());
        let freed = store.sweep(&keep_set).unwrap();
        assert_eq!(freed, drop.size);
        assert!(store.contains(&keep.digest).unwrap());
        assert!(!store.contains(&drop.digest).unwrap());
    }

    #[test]
    fn sweep_accepts_bare_hex_in_keep_set() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let r = store.install_from_bytes(b"keep").unwrap();
        let bare_hex = r.digest.strip_prefix("sha256:").unwrap().to_string();
        let mut keep = HashSet::new();
        keep.insert(bare_hex);
        let freed = store.sweep(&keep).unwrap();
        assert_eq!(freed, 0);
        assert!(store.contains(&r.digest).unwrap());
    }

    #[test]
    fn sweep_skips_inflight_tmp_files() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        std::fs::create_dir_all(d.path().join("sha256")).unwrap();
        let stray = d.path().join("sha256").join("abc.tmp.123.456");
        std::fs::write(&stray, b"inflight").unwrap();
        let freed = store.sweep(&HashSet::new()).unwrap();
        assert_eq!(freed, 0);
        assert!(stray.exists());
    }

    #[test]
    fn sweep_on_missing_root_is_no_op() {
        let d = tempdir();
        let store = ContentStore::new(d.path().join("never"));
        let freed = store.sweep(&HashSet::new()).unwrap();
        assert_eq!(freed, 0);
    }

    #[test]
    fn root_returns_configured_root() {
        let d = tempdir();
        let root = d.path().join("custom-root");
        let store = ContentStore::new(&root);
        assert_eq!(store.root(), root.as_path());
    }

    #[test]
    fn path_for_rejects_unsupported_digest_algorithm() {
        let store = ContentStore::new("/tmp");
        let err = store
            .path_for(&format!("md5:{}", "a".repeat(64)))
            .unwrap_err();
        assert!(format!("{err:#}").contains("unsupported"));
    }

    #[test]
    fn path_for_rejects_invalid_hex_payload() {
        let store = ContentStore::new("/tmp");
        let err = store.path_for("sha256:not-hex").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("64 lowercase hex"), "msg: {msg}");
    }

    #[test]
    fn install_from_reader_rename_failure_surfaces_context() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let bytes = b"rename-fail-stream";
        let hex = sha256_hex(bytes);
        std::fs::create_dir_all(d.path().join("sha256").join(&hex)).unwrap();
        let err = store
            .install_from_reader(std::io::Cursor::new(&bytes[..]))
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("rename"), "msg: {msg}");
    }

    #[test]
    fn install_from_bytes_rename_failure_surfaces_context() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let bytes = b"rename-fail-bulk";
        let hex = sha256_hex(bytes);
        std::fs::create_dir_all(d.path().join("sha256").join(&hex)).unwrap();
        let err = store.install_from_bytes(bytes).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("rename"), "msg: {msg}");
    }

    // O_EXCL against an existing file denies root too, unlike a mode bit.
    #[test]
    fn atomic_write_via_tmp_removes_a_tmp_it_could_not_open() {
        let d = tempdir();
        let dir = d.path().join("sha256");
        std::fs::create_dir(&dir).unwrap();
        let tmp = dir.join(".tmp.taken");
        std::fs::write(&tmp, b"someone-got-here-first").unwrap();

        let err = atomic_write_via_tmp(&dir.join("blob"), &tmp, b"locked-out").unwrap_err();

        let msg = format!("{err:#}");
        assert!(msg.contains("creating"), "msg: {msg}");
        assert!(!tmp.exists(), "the failed tmp must not survive");
        assert!(
            !dir.join("blob").exists(),
            "no blob may land when the tmp never opened"
        );
    }

    #[test]
    fn classify_sweep_entry_skips_inflight_tmp() {
        let keep: HashSet<&str> = HashSet::new();
        assert_eq!(
            classify_sweep_entry(std::ffi::OsStr::new("foo.tmp.123.456"), &keep),
            SweepAction::Skip,
        );
    }

    #[test]
    fn classify_sweep_entry_skips_kept_blob() {
        let hex = "a".repeat(64);
        let mut keep: HashSet<&str> = HashSet::new();
        keep.insert(hex.as_str());
        assert_eq!(
            classify_sweep_entry(std::ffi::OsStr::new(&hex), &keep),
            SweepAction::Skip,
        );
    }

    #[test]
    fn classify_sweep_entry_removes_unknown_blob() {
        let hex = "b".repeat(64);
        let keep: HashSet<&str> = HashSet::new();
        assert_eq!(
            classify_sweep_entry(std::ffi::OsStr::new(&hex), &keep),
            SweepAction::Remove,
        );
    }

    #[cfg(unix)]
    #[test]
    fn classify_sweep_entry_skips_non_utf8_name() {
        use std::os::unix::ffi::OsStrExt;
        let bad = std::ffi::OsStr::from_bytes(&[0xff, 0xfe, 0xfd]);
        let keep: HashSet<&str> = HashSet::new();
        assert_eq!(classify_sweep_entry(bad, &keep), SweepAction::Skip);
    }

    #[test]
    fn many_distinct_blobs_all_addressable() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let mut digests = Vec::new();
        for i in 0..100 {
            let payload = format!("blob-{i}").into_bytes();
            let r = store.install_from_bytes(&payload).unwrap();
            digests.push(r.digest);
        }
        for digest in &digests {
            assert!(store.contains(digest).unwrap(), "missing {digest}");
        }
    }

    #[test]
    fn concurrent_installs_of_distinct_blobs_never_collide_on_tmp_names() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        let store = &store;
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..64)
                .map(|i| {
                    scope.spawn(move || {
                        store.install_from_bytes(format!("concurrent-{i}").as_bytes())
                    })
                })
                .collect();
            for h in handles {
                assert!(h.join().unwrap().unwrap().path.is_file());
            }
        });
    }

    #[test]
    fn concurrent_installs_of_identical_bytes_all_succeed_and_dedup() {
        let d = tempdir();
        let store = ContentStore::new(d.path());
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..32)
                .map(|_| scope.spawn(|| store.install_from_bytes(b"same-bytes")))
                .collect();
            for h in handles {
                h.join().unwrap().unwrap();
            }
        });
        let count = std::fs::read_dir(d.path().join("sha256")).unwrap().count();
        assert_eq!(count, 1, "identical bytes must collapse to one blob");
    }
}
