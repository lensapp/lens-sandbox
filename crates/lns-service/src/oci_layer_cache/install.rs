use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub(super) trait Fs: Send + Sync {
    fn create_dir_all(&self, p: &Path) -> io::Result<()>;
    fn remove_file(&self, p: &Path) -> io::Result<()>;
    fn create_new(&self, p: &Path) -> io::Result<Box<dyn WritableFile>>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn is_file(&self, p: &Path) -> bool;
    fn is_dir(&self, p: &Path) -> bool;
    fn read(&self, p: &Path) -> io::Result<Vec<u8>>;
    fn read_dir_entries(&self, p: &Path) -> io::Result<Vec<DirEntryView>>;
}

pub(super) struct DirEntryView {
    pub name: std::ffi::OsString,
    pub path: PathBuf,
    pub size: u64,
}

pub(super) trait WritableFile: Write {
    fn sync_all(&self) -> io::Result<()>;
}

pub(super) struct RealFs;

impl Fs for RealFs {
    fn create_dir_all(&self, p: &Path) -> io::Result<()> {
        std::fs::create_dir_all(p)
    }
    fn remove_file(&self, p: &Path) -> io::Result<()> {
        std::fs::remove_file(p)
    }
    fn create_new(&self, p: &Path) -> io::Result<Box<dyn WritableFile>> {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(p)?;
        Ok(Box::new(f))
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }
    fn is_file(&self, p: &Path) -> bool {
        p.is_file()
    }
    fn is_dir(&self, p: &Path) -> bool {
        p.is_dir()
    }
    fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(p)
    }
    fn read_dir_entries(&self, p: &Path) -> io::Result<Vec<DirEntryView>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(p)? {
            let entry = entry?;
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(DirEntryView {
                name: entry.file_name(),
                path: entry.path(),
                size,
            });
        }
        Ok(out)
    }
}

impl WritableFile for std::fs::File {
    fn sync_all(&self) -> io::Result<()> {
        std::fs::File::sync_all(self)
    }
}

pub(super) fn install_from_bytes(
    fs: &dyn Fs,
    final_path: &Path,
    expected_digest: &str,
    bytes: &[u8],
) -> Result<()> {
    let tmp = prepare_tmp(fs, final_path)?;
    let outcome: Result<String> = (|| {
        let mut f = fs
            .create_new(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        f.write_all(bytes)
            .with_context(|| format!("writing {} bytes to {}", bytes.len(), tmp.display()))?;
        f.sync_all().context("fsync layer-cache blob")?;
        Ok(format!("{:x}", hasher.finalize()))
    })();
    let actual = cleanup_tmp_on_err(fs, &tmp, outcome)?;
    finalize_install(fs, &tmp, final_path, expected_digest, &actual)
}

pub(super) fn install_from_reader<R: Read>(
    fs: &dyn Fs,
    final_path: &Path,
    expected_digest: &str,
    mut reader: R,
) -> Result<()> {
    let tmp = prepare_tmp(fs, final_path)?;
    let outcome: Result<String> = (|| {
        let mut f = fs
            .create_new(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = reader
                .read(&mut buf)
                .with_context(|| format!("reading into {}", tmp.display()))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            f.write_all(&buf[..n])
                .with_context(|| format!("writing {n} bytes to {}", tmp.display()))?;
        }
        f.sync_all().context("fsync layer-cache blob")?;
        Ok(format!("{:x}", hasher.finalize()))
    })();
    let actual = cleanup_tmp_on_err(fs, &tmp, outcome)?;
    finalize_install(fs, &tmp, final_path, expected_digest, &actual)
}

fn prepare_tmp(fs: &dyn Fs, final_path: &Path) -> Result<PathBuf> {
    let parent = final_path
        .parent()
        .with_context(|| format!("install path {} has no parent", final_path.display()))?;
    fs.create_dir_all(parent)
        .with_context(|| format!("create_dir_all {}", parent.display()))?;
    let tmp = tmp_path(final_path);
    let _ = fs.remove_file(&tmp);
    Ok(tmp)
}

fn cleanup_tmp_on_err<T>(fs: &dyn Fs, tmp: &Path, outcome: Result<T>) -> Result<T> {
    match outcome {
        Ok(v) => Ok(v),
        Err(e) => {
            let _ = fs.remove_file(tmp);
            Err(e)
        }
    }
}

fn finalize_install(
    fs: &dyn Fs,
    tmp: &Path,
    final_path: &Path,
    expected_digest: &str,
    actual_hex: &str,
) -> Result<()> {
    let expected_hex = expected_digest
        .strip_prefix("sha256:")
        .with_context(|| format!("expected_digest {expected_digest:?} missing sha256: prefix"))?;
    if !ct_eq(actual_hex.as_bytes(), expected_hex.as_bytes()) {
        let _ = fs.remove_file(tmp);
        bail!("layer-cache install: digest mismatch — expected {expected_hex}, got {actual_hex}");
    }
    fs.rename(tmp, final_path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), final_path.display()))?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci_layer_cache::testfs::InMemoryFs;
    use std::io::Write;

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn install_writes_bytes_and_renames_into_place() {
        let fs = InMemoryFs::new();
        let bytes = b"hello";
        let target = PathBuf::from("/cache/sha256/abc");
        install_from_bytes(
            &fs,
            &target,
            &format!("sha256:{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap();
        assert_eq!(fs.read_bytes(&target).unwrap(), bytes);
        assert!(!fs.contains(&PathBuf::from("/cache/sha256/abc.tmp")));
        assert_eq!(
            fs.sync_calls(),
            vec![PathBuf::from("/cache/sha256/abc.tmp")]
        );
    }

    #[test]
    fn install_clears_pre_existing_tmp() {
        let fs = InMemoryFs::new();
        let target = PathBuf::from("/cache/blob");
        fs.put(&PathBuf::from("/cache/blob.tmp"), b"crashed");
        let bytes = b"fresh";
        install_from_bytes(
            &fs,
            &target,
            &format!("sha256:{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap();
        assert_eq!(fs.read_bytes(&target).unwrap(), bytes);
        assert!(!fs.contains(&PathBuf::from("/cache/blob.tmp")));
    }

    #[test]
    fn install_is_atomic_on_digest_mismatch() {
        let fs = InMemoryFs::new();
        let target = PathBuf::from("/cache/blob");
        let bytes = b"actual";
        let lies = format!("sha256:{}", "0".repeat(64));
        let err = install_from_bytes(&fs, &target, &lies, bytes).unwrap_err();
        assert!(format!("{err:#}").contains("digest mismatch"));
        assert!(!fs.contains(&target));
        assert!(!fs.contains(&PathBuf::from("/cache/blob.tmp")));
    }

    #[test]
    fn install_rejects_digest_missing_sha256_prefix() {
        let fs = InMemoryFs::new();
        let target = PathBuf::from("/cache/blob");
        let bytes = b"x";
        let err = install_from_bytes(&fs, &target, &sha256_hex(bytes), bytes).unwrap_err();
        assert!(format!("{err:#}").contains("sha256:"));
    }

    #[test]
    fn install_propagates_create_dir_all_failure() {
        let fs = InMemoryFs::new();
        fs.inject(|f| f.fail_create_dir_all = Some(io::ErrorKind::PermissionDenied));
        let bytes = b"x";
        let err = install_from_bytes(
            &fs,
            Path::new("/cache/blob"),
            &format!("sha256:{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("create_dir_all"));
    }

    #[test]
    fn install_rejects_path_with_no_parent() {
        let fs = InMemoryFs::new();
        let bytes = b"x";
        let err = install_from_bytes(
            &fs,
            Path::new("/"),
            &format!("sha256:{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("no parent"));
    }

    #[test]
    fn install_propagates_create_new_failure() {
        let fs = InMemoryFs::new();
        fs.inject(|f| f.fail_create_new = Some(io::ErrorKind::PermissionDenied));
        let bytes = b"x";
        let err = install_from_bytes(
            &fs,
            Path::new("/cache/blob"),
            &format!("sha256:{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("creating"));
    }

    #[test]
    fn install_propagates_write_failure() {
        let fs = InMemoryFs::new();
        fs.inject(|f| f.fail_write = Some(io::ErrorKind::BrokenPipe));
        let bytes = b"x";
        let err = install_from_bytes(
            &fs,
            Path::new("/cache/blob"),
            &format!("sha256:{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("writing"));
        assert!(!fs.contains(&PathBuf::from("/cache/blob.tmp")));
    }

    #[test]
    fn install_propagates_sync_failure() {
        let fs = InMemoryFs::new();
        fs.inject(|f| f.fail_sync = Some(io::ErrorKind::Other));
        let bytes = b"x";
        let err = install_from_bytes(
            &fs,
            Path::new("/cache/blob"),
            &format!("sha256:{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("fsync"));
        assert!(!fs.contains(&PathBuf::from("/cache/blob.tmp")));
    }

    #[test]
    fn install_propagates_rename_failure() {
        let fs = InMemoryFs::new();
        fs.inject(|f| f.fail_rename = Some(io::ErrorKind::PermissionDenied));
        let bytes = b"x";
        let err = install_from_bytes(
            &fs,
            Path::new("/cache/blob"),
            &format!("sha256:{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("rename"));
    }

    #[test]
    fn reader_install_round_trips_bytes() {
        let fs = InMemoryFs::new();
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let target = PathBuf::from("/cache/blob");
        install_from_reader(
            &fs,
            &target,
            &format!("sha256:{}", sha256_hex(&bytes)),
            &bytes[..],
        )
        .unwrap();
        assert_eq!(fs.read_bytes(&target).unwrap(), bytes);
    }

    #[test]
    fn reader_install_handles_empty_input() {
        let fs = InMemoryFs::new();
        let target = PathBuf::from("/cache/empty");
        install_from_reader(
            &fs,
            &target,
            &format!("sha256:{}", sha256_hex(b"")),
            std::io::empty(),
        )
        .unwrap();
        assert_eq!(fs.read_bytes(&target).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn reader_install_rejects_digest_mismatch() {
        let fs = InMemoryFs::new();
        let target = PathBuf::from("/cache/blob");
        let lies = format!("sha256:{}", "1".repeat(64));
        let err = install_from_reader(&fs, &target, &lies, &b"abc"[..]).unwrap_err();
        assert!(format!("{err:#}").contains("digest mismatch"));
        assert!(!fs.contains(&target));
        assert!(!fs.contains(&PathBuf::from("/cache/blob.tmp")));
    }

    #[test]
    fn reader_install_rejects_digest_missing_sha256_prefix() {
        let fs = InMemoryFs::new();
        let target = PathBuf::from("/cache/blob");
        let bytes = b"abc";
        let err = install_from_reader(&fs, &target, &sha256_hex(bytes), &bytes[..]).unwrap_err();
        assert!(format!("{err:#}").contains("sha256:"));
    }

    #[test]
    fn reader_install_rejects_path_with_no_parent() {
        let fs = InMemoryFs::new();
        let bytes = b"x";
        let err = install_from_reader(
            &fs,
            Path::new("/"),
            &format!("sha256:{}", sha256_hex(bytes)),
            &bytes[..],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("no parent"));
    }

    #[test]
    fn reader_install_propagates_create_dir_all_failure() {
        let fs = InMemoryFs::new();
        fs.inject(|f| f.fail_create_dir_all = Some(io::ErrorKind::PermissionDenied));
        let bytes = b"x";
        let err = install_from_reader(
            &fs,
            Path::new("/cache/blob"),
            &format!("sha256:{}", sha256_hex(bytes)),
            &bytes[..],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("create_dir_all"));
    }

    #[test]
    fn reader_install_propagates_create_new_failure() {
        let fs = InMemoryFs::new();
        fs.inject(|f| f.fail_create_new = Some(io::ErrorKind::PermissionDenied));
        let bytes = b"x";
        let err = install_from_reader(
            &fs,
            Path::new("/cache/blob"),
            &format!("sha256:{}", sha256_hex(bytes)),
            &bytes[..],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("creating"));
    }

    #[test]
    fn reader_install_propagates_write_failure() {
        let fs = InMemoryFs::new();
        fs.inject(|f| f.fail_write = Some(io::ErrorKind::BrokenPipe));
        let bytes = b"x";
        let err = install_from_reader(
            &fs,
            Path::new("/cache/blob"),
            &format!("sha256:{}", sha256_hex(bytes)),
            &bytes[..],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("writing"));
        assert!(!fs.contains(&PathBuf::from("/cache/blob.tmp")));
    }

    #[test]
    fn reader_install_propagates_sync_failure() {
        let fs = InMemoryFs::new();
        fs.inject(|f| f.fail_sync = Some(io::ErrorKind::Other));
        let bytes = b"x";
        let err = install_from_reader(
            &fs,
            Path::new("/cache/blob"),
            &format!("sha256:{}", sha256_hex(bytes)),
            &bytes[..],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("fsync"));
        assert!(!fs.contains(&PathBuf::from("/cache/blob.tmp")));
    }

    #[test]
    fn reader_install_propagates_rename_failure() {
        let fs = InMemoryFs::new();
        fs.inject(|f| f.fail_rename = Some(io::ErrorKind::PermissionDenied));
        let bytes = b"x";
        let err = install_from_reader(
            &fs,
            Path::new("/cache/blob"),
            &format!("sha256:{}", sha256_hex(bytes)),
            &bytes[..],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("rename"));
    }

    #[test]
    fn reader_install_propagates_mid_stream_read_failure() {
        struct ErrAfterReader {
            yielded: bool,
        }
        impl Read for ErrAfterReader {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if !self.yielded {
                    self.yielded = true;
                    buf[..3].copy_from_slice(b"abc");
                    return Ok(3);
                }
                Err(io::Error::from(io::ErrorKind::ConnectionReset))
            }
        }
        let fs = InMemoryFs::new();
        let err = install_from_reader(
            &fs,
            Path::new("/cache/blob"),
            &format!("sha256:{}", "0".repeat(64)),
            ErrAfterReader { yielded: false },
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("reading into"));
        assert!(!fs.contains(&PathBuf::from("/cache/blob.tmp")));
    }

    #[test]
    fn tmp_path_appends_dot_tmp_to_filename() {
        assert_eq!(
            tmp_path(Path::new("/a/b/blob")),
            PathBuf::from("/a/b/blob.tmp")
        );
    }

    #[test]
    fn tmp_path_handles_path_without_filename() {
        let p = tmp_path(Path::new("/"));
        assert!(p.to_string_lossy().ends_with(".tmp"));
    }

    #[test]
    fn ct_eq_rejects_different_lengths() {
        assert!(!ct_eq(b"abc", b"abcd"));
    }

    #[test]
    fn ct_eq_accepts_equal_byte_strings() {
        assert!(ct_eq(b"abcdef", b"abcdef"));
    }

    #[test]
    fn ct_eq_rejects_single_byte_diff() {
        assert!(!ct_eq(b"abcdef", b"abcdeg"));
        assert!(!ct_eq(b"abcdef", b"Xbcdef"));
    }

    #[test]
    fn install_ignores_stale_tmp_cleanup_failure() {
        let fs = InMemoryFs::new();
        fs.inject(|f| f.fail_remove_file = Some(io::ErrorKind::PermissionDenied));
        let bytes = b"resilient";
        let target = PathBuf::from("/cache/blob");
        install_from_bytes(
            &fs,
            &target,
            &format!("sha256:{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap();
        assert_eq!(fs.read_bytes(&target).unwrap(), bytes);
    }

    #[test]
    fn install_surfaces_create_new_already_exists_when_cleanup_swallowed() {
        let fs = InMemoryFs::new();
        let target = PathBuf::from("/cache/blob");
        fs.put(&PathBuf::from("/cache/blob.tmp"), b"in flight elsewhere");
        fs.inject(|f| f.fail_remove_file = Some(io::ErrorKind::PermissionDenied));
        let bytes = b"never lands";
        let err = install_from_bytes(
            &fs,
            &target,
            &format!("sha256:{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("creating"), "unexpected error: {msg}");
    }

    #[test]
    fn fake_file_flush_is_noop() {
        let fs = InMemoryFs::new();
        let mut f = fs.create_new(Path::new("/scratch")).unwrap();
        f.write_all(b"hi").unwrap();
        f.flush().unwrap();
    }

    #[test]
    fn real_fs_round_trips_bytes_through_tempdir() {
        let d = tempfile::TempDir::new().unwrap();
        let target = d.path().join("real-blob");
        let bytes = b"real bytes through real fs";
        install_from_bytes(
            &RealFs,
            &target,
            &format!("sha256:{}", sha256_hex(bytes)),
            bytes,
        )
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
    }

    fn err_kind<T>(r: io::Result<T>) -> io::ErrorKind {
        r.map(|_| ()).expect_err("expected an io::Error").kind()
    }

    #[test]
    fn real_fs_create_new_errors_when_target_exists() {
        let d = tempfile::TempDir::new().unwrap();
        let p = d.path().join("already-here");
        std::fs::write(&p, b"prior").unwrap();
        assert_eq!(
            err_kind(RealFs.create_new(&p)),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn real_fs_remove_file_errors_when_missing() {
        let d = tempfile::TempDir::new().unwrap();
        assert_eq!(
            err_kind(RealFs.remove_file(&d.path().join("absent"))),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn real_fs_rename_errors_when_source_missing() {
        let d = tempfile::TempDir::new().unwrap();
        assert_eq!(
            err_kind(RealFs.rename(&d.path().join("missing"), &d.path().join("dst"))),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn real_fs_is_file_distinguishes_files_dirs_and_missing() {
        let d = tempfile::TempDir::new().unwrap();
        let file = d.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        let dir = d.path().join("sub");
        std::fs::create_dir(&dir).unwrap();
        assert!(RealFs.is_file(&file));
        assert!(!RealFs.is_file(&dir));
        assert!(!RealFs.is_file(&d.path().join("absent")));
    }

    #[test]
    fn real_fs_is_dir_distinguishes_dirs_files_and_missing() {
        let d = tempfile::TempDir::new().unwrap();
        let file = d.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        let dir = d.path().join("sub");
        std::fs::create_dir(&dir).unwrap();
        assert!(RealFs.is_dir(&dir));
        assert!(!RealFs.is_dir(&file));
        assert!(!RealFs.is_dir(&d.path().join("absent")));
    }

    #[test]
    fn real_fs_read_round_trips_bytes() {
        let d = tempfile::TempDir::new().unwrap();
        let p = d.path().join("blob");
        let bytes = b"some bytes";
        std::fs::write(&p, bytes).unwrap();
        assert_eq!(RealFs.read(&p).unwrap(), bytes);
    }

    #[test]
    fn real_fs_read_errors_when_missing() {
        let d = tempfile::TempDir::new().unwrap();
        assert_eq!(
            err_kind(RealFs.read(&d.path().join("absent"))),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn real_fs_read_dir_entries_returns_names_paths_and_sizes() {
        let d = tempfile::TempDir::new().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("bb");
        std::fs::write(&a, b"hi").unwrap();
        std::fs::write(&b, b"hello").unwrap();
        let mut entries = RealFs.read_dir_entries(d.path()).unwrap();
        entries.sort_by(|x, y| x.name.cmp(&y.name));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, std::ffi::OsString::from("a"));
        assert_eq!(entries[0].path, a);
        assert_eq!(entries[0].size, 2);
        assert_eq!(entries[1].name, std::ffi::OsString::from("bb"));
        assert_eq!(entries[1].path, b);
        assert_eq!(entries[1].size, 5);
    }

    #[test]
    fn real_fs_read_dir_entries_errors_on_missing_dir() {
        let d = tempfile::TempDir::new().unwrap();
        assert_eq!(
            err_kind(RealFs.read_dir_entries(&d.path().join("absent"))),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn real_fs_create_dir_all_errors_when_parent_is_a_file() {
        let d = tempfile::TempDir::new().unwrap();
        let blocker = d.path().join("blocker");
        std::fs::write(&blocker, b"file-not-dir").unwrap();
        RealFs
            .create_dir_all(&blocker.join("under"))
            .expect_err("create_dir_all under a regular file must fail");
    }

    #[test]
    fn fake_rename_returns_not_found_when_source_missing() {
        let fs = InMemoryFs::new();
        assert_eq!(
            err_kind(fs.rename(Path::new("/missing"), Path::new("/dst"))),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn fake_remove_file_returns_not_found_when_missing() {
        let fs = InMemoryFs::new();
        assert_eq!(
            err_kind(fs.remove_file(Path::new("/missing"))),
            io::ErrorKind::NotFound
        );
    }
}
